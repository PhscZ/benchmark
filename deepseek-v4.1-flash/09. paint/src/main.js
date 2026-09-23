import { History } from './history.js';
import { loadImageFile, ImageLoadError } from './loader.js';
import { Viewport } from './viewport.js';
import { PaintEngine, SHAPE_TOOLS, PAINT_TOOLS } from './paint.js';
import { CropSession, HANDLE_CURSORS } from './crop.js';
import { AdjustmentPreview } from './adjustments.js';
import { BusyState } from './busy.js';
import {
  rotate90, rotate180, mirrorHorizontal, mirrorVertical, resizeDocument, validateDimensions, lockAspect,
} from './transform.js';
import { exportDocument, downloadBlob, verifyEncodedDimensions, EXPORT_FORMATS } from './export.js';
import {
  BRUSH_SIZE_DEFAULT, BRUSH_SIZE_MAX, BRUSH_SIZE_MIN, DEFAULT_COLOR, EXPORT_BACKGROUND_DEFAULT,
  HISTORY_BUDGET_BYTES, RESIZE_MIN, ZOOM_MAX, ZOOM_MIN, ZOOM_STEP,
} from './config.js';
import { clamp, formatBytes, nextFrame, rgbToHex } from './util.js';

const $ = (id) => document.getElementById(id);

const TOOL_SHORTCUTS = {
  b: 'brush', e: 'eraser', l: 'line', r: 'rect', o: 'ellipse', i: 'picker', c: 'crop', h: 'pan',
};

/** Tools that paint into the document. */
const PAINTING_TOOLS = new Set(['brush', 'eraser', 'line', 'rect', 'ellipse']);

function confirmDialog({ title, body, confirmLabel = 'Discard and continue' }) {
  return new Promise((resolve) => {
    const root = $('modalRoot');
    $('modalTitle').textContent = title;
    $('modalBody').textContent = body;
    $('modalConfirm').textContent = confirmLabel;
    root.hidden = false;
    const cleanup = (result) => {
      root.hidden = true;
      $('modalConfirm').removeEventListener('click', onConfirm);
      $('modalCancel').removeEventListener('click', onCancel);
      document.removeEventListener('keydown', onKey, true);
      resolve(result);
    };
    const onConfirm = () => cleanup(true);
    const onCancel = () => cleanup(false);
    const onKey = (event) => {
      if (event.key === 'Escape') { event.stopPropagation(); cleanup(false); }
      if (event.key === 'Enter') { event.stopPropagation(); cleanup(true); }
    };
    $('modalConfirm').addEventListener('click', onConfirm);
    $('modalCancel').addEventListener('click', onCancel);
    document.addEventListener('keydown', onKey, true);
    $('modalConfirm').focus();
  });
}

class Editor {
  constructor() {
    /** @type {RasterDocument|null} */
    this.doc = null;
    this.history = null;
    this.busy = new BusyState();
    this.paint = new PaintEngine();
    this.crop = new CropSession();
    this.adjustments = new AdjustmentPreview();

    this.tool = 'brush';
    this.brushSize = BRUSH_SIZE_DEFAULT;
    this.brushColor = DEFAULT_COLOR;
    this.opacity = 1;
    this.shapeMode = 'outline';
    this.aspectPresetId = 'free';

    this.dirty = false;
    this.hasImage = false;
    this.statusMessage = 'Ready. Open an image to start.';
    this.statusKind = 'info';
    this.cursorImagePoint = null;
    this.pointerId = null;
    this.spaceHeld = false;
    this.dragState = null;
    this.lastExport = null;
    this.adjustTimer = 0;
    this.adjustRun = null;
    this.resizeLocked = $('resizeLock').checked;
    this.evictionNoticeShown = false;
    /** Undo history memory budget; see HISTORY_BUDGET_BYTES and README. */
    this.historyBudgetBytes = HISTORY_BUDGET_BYTES;

    this.viewport = new Viewport($('viewportCanvas'), $('overlayCanvas'), $('stage'));
    this.viewport.overlay = (ctx, viewport) => this.drawOverlay(ctx, viewport);
    this.viewport.onTransform = () => this.updateZoomReadout();
  }

  // ------------------------------------------------------------------ setup
  init() {
    this.bindFileInputs();
    this.bindToolbar();
    this.bindTools();
    this.bindPaintOptions();
    this.bindCropOptions();
    this.bindResize();
    this.bindTransforms();
    this.bindAdjustments();
    this.bindExport();
    this.bindCanvas();
    this.bindKeyboard();
    this.bindDragAndDrop();

    this.busy.subscribe(() => {
      this.updateUi();
      this.renderStatus();
    });
    this.updateUi();
    this.renderStatus();
    window.addEventListener('beforeunload', (event) => {
      if (!this.dirty) return;
      event.preventDefault();
      event.returnValue = '';
    });
  }

  bindFileInputs() {
    $('openButton').addEventListener('click', () => $('openInput').click());
    $('openInput').addEventListener('change', async (event) => {
      const file = event.target.files?.[0];
      event.target.value = '';
      if (file) await this.openFile(file);
    });
    $('reopenExportButton').addEventListener('click', () => this.reopenLastExport());
  }

  bindToolbar() {
    $('undoButton').addEventListener('click', () => this.undo());
    $('redoButton').addEventListener('click', () => this.redo());
    $('zoomInButton').addEventListener('click', () => this.viewport.zoomStep(1));
    $('zoomOutButton').addEventListener('click', () => this.viewport.zoomStep(-1));
    $('zoomFitButton').addEventListener('click', () => this.viewport.fit());
    $('zoomActualButton').addEventListener('click', () => this.viewport.actualSize());
  }

  bindTools() {
    for (const button of $('toolButtons').querySelectorAll('button[data-tool]')) {
      button.addEventListener('click', () => this.setTool(button.dataset.tool));
    }
  }

  bindPaintOptions() {
    const size = $('brushSize');
    size.addEventListener('input', () => {
      this.brushSize = Number(size.value);
      $('brushSizeValue').textContent = `${this.brushSize} px`;
      this.viewport.requestOverlayRender();
    });
    const color = $('brushColor');
    color.addEventListener('input', () => { this.brushColor = color.value; });
    const opacity = $('opacity');
    opacity.addEventListener('input', () => {
      this.opacity = Number(opacity.value) / 100;
      $('opacityValue').textContent = `${Math.round(this.opacity * 100)}%`;
    });
    $('shapeMode').addEventListener('change', (event) => { this.shapeMode = event.target.value; });
  }

  bindCropOptions() {
    $('aspectPreset').addEventListener('change', (event) => {
      this.aspectPresetId = event.target.value;
      if (!this.doc) return;
      this.crop.setAspectPreset(this.aspectPresetId, this.doc);
      this.syncCropInputs();
      this.viewport.requestOverlayRender();
    });
    for (const [id, field] of [['cropX', 'x'], ['cropY', 'y'], ['cropW', 'width'], ['cropH', 'height']]) {
      $(id).addEventListener('change', () => {
        if (!this.doc) return;
        const values = this.readCropInputs();
        const result = this.crop.setNumeric(this.doc, values);
        this.showCropError(result.ok ? '' : result.error);
        if (result.ok) this.viewport.requestOverlayRender();
      });
    }
    $('cropSelectAll').addEventListener('click', () => {
      if (!this.doc) return;
      this.crop.reset(this.doc);
      this.syncCropInputs();
      this.showCropError('');
      this.viewport.requestOverlayRender();
    });
    $('cropApply').addEventListener('click', () => this.applyCrop());
    $('cropCancel').addEventListener('click', () => this.cancelCrop());
  }

  bindResize() {
    const syncLockedAxis = (changed) => {
      if (!this.resizeLocked || !this.doc) return;
      const value = Number($(changed === 'width' ? 'resizeWidth' : 'resizeHeight').value);
      if (!Number.isInteger(value) || value < RESIZE_MIN) return;
      const derived = lockAspect(this.doc.width, this.doc.height, changed, value);
      $('resizeWidth').value = String(derived.width);
      $('resizeHeight').value = String(derived.height);
    };
    $('resizeWidth').addEventListener('input', () => syncLockedAxis('width'));
    $('resizeHeight').addEventListener('input', () => syncLockedAxis('height'));
    $('resizeLock').addEventListener('change', (event) => { this.resizeLocked = event.target.checked; });
    $('resizeApply').addEventListener('click', () => this.applyResize());
  }

  bindTransforms() {
    $('rotateCw').addEventListener('click', () => this.runTransform('Rotate 90° CW', (doc) => rotate90(doc, 1)));
    $('rotateCcw').addEventListener('click', () => this.runTransform('Rotate 90° CCW', (doc) => rotate90(doc, -1)));
    $('rotate180').addEventListener('click', () => this.runTransform('Rotate 180°', (doc) => rotate180(doc)));
    $('mirrorH').addEventListener('click', () => this.runTransform('Mirror horizontally', (doc) => mirrorHorizontal(doc)));
    $('mirrorV').addEventListener('click', () => this.runTransform('Mirror vertically', (doc) => mirrorVertical(doc)));
  }

  bindAdjustments() {
    for (const id of ['brightness', 'contrast']) {
      $(id).addEventListener('input', () => {
        $(`${id}Value`).textContent = $(id).value;
        this.scheduleAdjustPreview();
      });
    }
    $('grayscaleToggle').addEventListener('change', () => this.scheduleAdjustPreview());
    $('invertToggle').addEventListener('change', () => this.scheduleAdjustPreview());
    $('adjustApply').addEventListener('click', () => this.applyAdjustments());
    $('adjustCancel').addEventListener('click', () => this.cancelAdjustments());
    $('adjustReset').addEventListener('click', () => {
      $('brightness').value = '0';
      $('contrast').value = '0';
      $('brightnessValue').textContent = '0';
      $('contrastValue').textContent = '0';
      $('grayscaleToggle').checked = false;
      $('invertToggle').checked = false;
      this.scheduleAdjustPreview(0);
    });
  }

  bindExport() {
    const format = $('exportFormat');
    const syncFormatRows = () => {
      const isJpeg = format.value === 'jpeg';
      $('jpegQualityRow').hidden = !isJpeg;
      $('exportBackgroundRow').hidden = !isJpeg;
    };
    format.addEventListener('change', syncFormatRows);
    syncFormatRows();
    $('jpegQuality').addEventListener('input', () => {
      $('jpegQualityValue').textContent = `${$('jpegQuality').value}%`;
    });
    $('exportRunButton').addEventListener('click', () => this.runExport());
  }

  bindCanvas() {
    const canvas = this.viewport.canvas;
    canvas.addEventListener('pointerdown', (event) => this.onPointerDown(event));
    canvas.addEventListener('pointermove', (event) => this.onPointerMove(event));
    canvas.addEventListener('pointerup', (event) => this.onPointerUp(event));
    canvas.addEventListener('pointercancel', (event) => this.onPointerUp(event));
    canvas.addEventListener('pointerleave', () => {
      this.cursorImagePoint = null;
      this.viewport.requestOverlayRender();
    });
    canvas.addEventListener('wheel', (event) => this.onWheel(event), { passive: false });
    canvas.addEventListener('contextmenu', (event) => event.preventDefault());
  }

  bindKeyboard() {
    window.addEventListener('keydown', (event) => this.onKeyDown(event));
    window.addEventListener('keyup', (event) => {
      if (event.code === 'Space') { this.spaceHeld = false; this.updateCursorStyle(); }
    });
    window.addEventListener('blur', () => { this.spaceHeld = false; this.updateCursorStyle(); });
  }

  bindDragAndDrop() {
    const overlay = $('dropOverlay');
    let depth = 0;
    window.addEventListener('dragenter', (event) => {
      if (!event.dataTransfer?.types?.includes('Files')) return;
      event.preventDefault();
      depth += 1;
      overlay.hidden = false;
    });
    window.addEventListener('dragover', (event) => {
      if (!event.dataTransfer?.types?.includes('Files')) return;
      event.preventDefault();
      event.dataTransfer.dropEffect = 'copy';
    });
    window.addEventListener('dragleave', () => {
      depth = Math.max(0, depth - 1);
      if (depth === 0) overlay.hidden = true;
    });
    window.addEventListener('drop', async (event) => {
      if (!event.dataTransfer?.files?.length) return;
      event.preventDefault();
      depth = 0;
      overlay.hidden = true;
      await this.openFile(event.dataTransfer.files[0]);
    });
  }

  // ------------------------------------------------------------- document
  /**
   * Open a file, warning first when the current image has unexported changes.
   * @param {File} file
   */
  async openFile(file) {
    if (this.busy.busy) {
      this.setStatus('An operation is still running; try again in a moment.', 'warn');
      return false;
    }
    if (this.hasImage && this.dirty) {
      const proceed = await confirmDialog({
        title: 'Discard unexported changes?',
        body: `"${file.name}" will replace the current image, and it has changes that have not been exported yet.`,
        confirmLabel: 'Discard and open',
      });
      if (!proceed) {
        this.setStatus('Open cancelled; the current image is unchanged.', 'info');
        return false;
      }
    }
    return this.busy.run('Opening image', async () => {
      try {
        const { doc, info } = await loadImageFile(file);
        this.adoptDocument(doc, info);
        const warn = info.warnings.length ? ` ${info.warnings.join(' ')}` : '';
        this.setStatus(
          `Opened ${info.name} — ${info.width} x ${info.height} px, ${formatBytes(info.bytes)} (${info.format.toUpperCase()}).${warn}`,
          info.warnings.length ? 'warn' : 'ok',
        );
        return true;
      } catch (error) {
        const message = error instanceof ImageLoadError ? error.message : `Could not open "${file.name}": ${error.message}`;
        this.setStatus(`${message} The current image was kept.`, 'error');
        return false;
      }
    });
  }

  /** Install a freshly decoded document, releasing everything it replaces. */
  adoptDocument(doc, info) {
    this.paint.release();
    this.adjustments.release();
    this.crop.clear();
    this.doc = doc;
    this.history = new History(doc, {
      budgetBytes: this.historyBudgetBytes,
      onEvict: (details) => this.onHistoryEvict(details),
    });
    this.history.reset();
    this.clearHistoryNotice();
    this.dirty = false;
    this.hasImage = true;
    this.lastInfo = info;
    this.viewport.setLayers([]);
    this.viewport.setDocument(doc);
    $('resizeWidth').value = String(doc.width);
    $('resizeHeight').value = String(doc.height);
    $('exportFilename').value = defaultExportName(info.name);
    this.crop.reset(doc);
    this.syncCropInputs();
    this.showCropError('');
    this.showResizeError('');
    this.showExportError('');
    this.updateUi();
  }

  /**
   * Record the current document content as an undo state.
   * @param {string} label
   */
  commit(label) {
    if (!this.doc) return;
    this.history.commit(label);
    this.dirty = true;
    this.afterDocumentChange();
  }

  /** Re-render after any pixel or dimension change. */
  afterDocumentChange() {
    this.paint.release();
    this.viewport.setLayers(this.currentLayers());
    this.viewport.clampPan();
    this.viewport.drawAll();
    this.updateUi();
  }

  currentLayers() {
    const layers = [];
    const stroke = this.paint.previewCanvas;
    if (stroke) layers.push({ canvas: stroke, opacity: 1 });
    const preview = this.adjustments.previewCanvas;
    if (preview) layers.push({ canvas: preview, opacity: 1 });
    return layers;
  }

  undo() {
    if (!this.history?.canUndo) return;
    const label = this.history.nextUndoLabel;
    this.history.undo();
    this.dirty = true;
    this.afterDocumentChange();
    this.afterHistoryRestore();
    this.setStatus(`Undo: ${label}.`, 'info');
  }

  redo() {
    if (!this.history?.canRedo) return;
    const label = this.history.nextRedoLabel;
    this.history.redo();
    this.dirty = true;
    this.afterDocumentChange();
    this.afterHistoryRestore();
    this.setStatus(`Redo: ${label}.`, 'info');
  }

  /** Keep the crop selection valid after the document dimensions changed. */
  afterHistoryRestore() {
    if (this.crop.active) this.crop.clamp(this.doc);
    this.syncCropInputs();
    this.showCropError('');
  }

  onHistoryEvict({ evicted, memoryBytes, states }) {
    this.evictionNoticeShown = true;
    const notice = $('notice');
    notice.textContent = `History limit reached: ${evicted} oldest undo state${evicted === 1 ? '' : 's'} discarded to stay within ${formatBytes(this.historyBudgetBytes)} (${states} states, ${formatBytes(memoryBytes)} held).`;
    notice.hidden = false;
  }

  clearHistoryNotice() {
    this.evictionNoticeShown = false;
    const notice = $('notice');
    notice.textContent = '';
    notice.hidden = true;
  }

  // ------------------------------------------------------------------ tools
  setTool(tool) {
    if (!PAINT_TOOLS.has(tool) && tool !== 'picker' && tool !== 'crop' && tool !== 'pan') return;
    if (tool !== 'crop') {
      this.crop.endDrag();
      if (this.crop.active) { this.crop.clear(); }
    } else if (this.doc && !this.crop.active) {
      this.crop.reset(this.doc);
      this.syncCropInputs();
    }
    this.tool = tool;
    this.paint.cancel();
    this.viewport.setLayers(this.currentLayers());
    this.updateUi();
    this.viewport.requestRender();
    this.setStatus(`${toolLabel(tool)} selected.`, 'info');
  }

  // --------------------------------------------------------------- pointers
  /** Pointer capture can fail when the pointer is already gone; never fatal. */
  capturePointer(pointerId) {
    try {
      this.viewport.canvas.setPointerCapture(pointerId);
      this.pointerId = pointerId;
    } catch {
      this.pointerId = null;
    }
  }

  onPointerDown(event) {
    if (!this.doc || this.busy.busy || this.adjustments.active) return;
    const point = this.viewport.screenToImage(event.clientX, event.clientY);
    this.viewport.canvas.focus({ preventScroll: true });

    const wantsPan = this.tool === 'pan' || event.button === 1 || this.spaceHeld;
    if (wantsPan) {
      event.preventDefault();
      this.dragState = { kind: 'pan', lastX: event.clientX, lastY: event.clientY };
      this.capturePointer(event.pointerId);
      this.updateCursorStyle(true);
      return;
    }
    if (event.button !== 0) return;

    if (this.tool === 'crop') {
      const mode = this.crop.hitTest(point, CropSession.hitTolerance(this.viewport), this.doc);
      this.crop.beginDrag(mode, point, this.doc);
      this.dragState = { kind: 'crop' };
      this.capturePointer(event.pointerId);
      return;
    }

    if (this.tool === 'picker') {
      this.pickColor(point);
      return;
    }

    if (!PAINTING_TOOLS.has(this.tool)) return;
    event.preventDefault();
    this.capturePointer(event.pointerId);
    this.paint.begin({
      doc: this.doc,
      tool: this.tool,
      point,
      color: this.brushColor,
      size: this.brushSize,
      opacity: this.opacity,
      shapeMode: this.shapeMode,
    });
    this.dragState = { kind: 'paint' };
    this.viewport.setLayers(this.currentLayers());
    this.viewport.requestRender();
  }

  onPointerMove(event) {
    if (!this.doc) return;
    const point = this.viewport.screenToImage(event.clientX, event.clientY);
    this.cursorImagePoint = point;

    if (this.dragState?.kind === 'pan') {
      this.viewport.panBy(event.clientX - this.dragState.lastX, event.clientY - this.dragState.lastY);
      this.dragState.lastX = event.clientX;
      this.dragState.lastY = event.clientY;
      return;
    }
    if (this.dragState?.kind === 'crop') {
      this.crop.drag(point, this.doc, true);
      this.syncCropInputs();
      this.viewport.requestOverlayRender();
      return;
    }
    if (this.dragState?.kind === 'paint') {
      this.paint.move(point, event.shiftKey);
      this.viewport.requestRender();
      return;
    }
    if (this.tool === 'crop') this.updateCursorStyle(false, point);
    this.viewport.requestOverlayRender();
  }

  onPointerUp(event) {
    if (!this.dragState) return;
    const kind = this.dragState.kind;
    this.dragState = null;
    if (this.pointerId !== null) {
      try { this.viewport.canvas.releasePointerCapture(this.pointerId); } catch { /* already released */ }
      this.pointerId = null;
    }
    if (kind === 'pan') {
      this.updateCursorStyle(false);
      return;
    }
    if (kind === 'crop') {
      this.crop.endDrag();
      this.syncCropInputs();
      const check = this.crop.validate(this.doc);
      this.showCropError(check.ok ? '' : check.error);
      this.viewport.requestOverlayRender();
      return;
    }
    if (kind === 'paint') {
      const result = this.paint.end();
      this.viewport.setLayers(this.currentLayers());
      if (result.changed) {
        this.commit(result.label);
        this.setStatus(`${result.label} applied.`, 'info');
      } else {
        this.viewport.requestRender();
      }
    }
  }

  onWheel(event) {
    if (!this.doc) return;
    event.preventDefault();
    if (event.shiftKey) {
      this.viewport.panBy(-event.deltaY, 0);
      return;
    }
    const rect = this.viewport.canvas.getBoundingClientRect();
    const factor = event.deltaY < 0 ? ZOOM_STEP : 1 / ZOOM_STEP;
    this.viewport.zoomBy(factor, event.clientX - rect.left, event.clientY - rect.top);
  }

  pickColor(point) {
    const sample = this.doc.sample(point.x, point.y);
    if (!sample) {
      this.setStatus('Eyedropper: that point is outside the image.', 'warn');
      return;
    }
    const hex = rgbToHex(sample.r, sample.g, sample.b);
    $('brushColor').value = hex;
    this.brushColor = hex;
    $('pickerSwatch').style.backgroundColor = hex;
    $('pickerValue').textContent = `${hex} · ${sample.a === 255 ? 'opaque' : `alpha ${sample.a}`}`;
    this.setStatus(`Eyedropper: sampled ${hex} at ${Math.floor(point.x)}, ${Math.floor(point.y)}.`, 'info');
  }

  updateCursorStyle(panning = false, point = null) {
    const canvas = this.viewport.canvas;
    if (panning) { canvas.style.cursor = 'grabbing'; return; }
    if (this.tool === 'pan' || this.spaceHeld) { canvas.style.cursor = 'grab'; return; }
    if (this.tool === 'crop') {
      const target = point ?? this.cursorImagePoint;
      if (!target || !this.crop.active) { canvas.style.cursor = 'crosshair'; return; }
      const mode = this.crop.hitTest(target, CropSession.hitTolerance(this.viewport), this.doc);
      canvas.style.cursor = HANDLE_CURSORS[mode] ?? 'crosshair';
      return;
    }
    canvas.style.cursor = this.tool === 'picker' ? 'crosshair' : 'none';
  }

  drawOverlay(ctx, viewport) {
    if (!this.doc) return;
    if (this.tool === 'crop') this.crop.drawOverlay(ctx, viewport);
    const point = this.cursorImagePoint;
    if (!point || this.dragState?.kind === 'pan' || this.tool === 'crop' || this.tool === 'pan') return;
    if (PAINTING_TOOLS.has(this.tool)) {
      PaintEngine.drawCursor(ctx, viewport, point, this.brushSize);
    } else if (this.tool === 'picker') {
      const screen = viewport.imageToScreen(point.x, point.y);
      ctx.save();
      ctx.strokeStyle = '#fff';
      ctx.lineWidth = 1;
      ctx.beginPath();
      ctx.arc(screen.x, screen.y, 5, 0, Math.PI * 2);
      ctx.stroke();
      ctx.restore();
    }
  }

  // ------------------------------------------------------------------- crop
  readCropInputs() {
    return {
      x: Number($('cropX').value),
      y: Number($('cropY').value),
      width: Number($('cropW').value),
      height: Number($('cropH').value),
    };
  }

  syncCropInputs() {
    if (!this.crop.rect) return;
    const rect = this.crop.rect;
    $('cropX').value = String(Math.round(rect.x));
    $('cropY').value = String(Math.round(rect.y));
    $('cropW').value = String(Math.round(rect.width));
    $('cropH').value = String(Math.round(rect.height));
  }

  showCropError(message) {
    $('cropError').textContent = message ?? '';
  }

  async applyCrop() {
    if (!this.doc || this.busy.busy || this.adjustments.active) return;
    // The numeric fields are authoritative: a value the user typed that could not
    // be represented as a rectangle must never be silently replaced by the last
    // valid selection.
    const fields = this.crop.setNumeric(this.doc, this.readCropInputs());
    const check = fields.ok ? this.crop.validate(this.doc) : { ok: false, error: fields.error };
    if (!check.ok) {
      this.showCropError(check.error);
      this.setStatus(`Crop rejected: ${check.error}`, 'error');
      return;
    }
    const { width, height } = check.rect;
    await this.busy.run('Cropping', async () => {
      const result = this.crop.apply(this.doc);
      if (!result.ok) {
        this.showCropError(result.error);
        this.setStatus(`Crop rejected: ${result.error}`, 'error');
        return;
      }
      this.showCropError('');
      this.commit(`Crop to ${width} x ${height}`);
      this.crop.reset(this.doc);
      this.syncCropInputs();
      this.setStatus(`Cropped to ${result.width} x ${result.height} px.`, 'ok');
    });
  }

  cancelCrop() {
    if (!this.doc) return;
    this.crop.reset(this.doc);
    this.syncCropInputs();
    this.showCropError('');
    this.viewport.requestOverlayRender();
    this.setStatus('Crop cancelled; the selection was reset to the full image.', 'info');
  }

  // -------------------------------------------------------------- transforms
  async runTransform(label, operation) {
    if (!this.doc || this.busy.busy || this.adjustments.active) return;
    await this.busy.run(label, async () => {
      operation(this.doc);
      if (this.crop.active) { this.crop.reset(this.doc); this.syncCropInputs(); }
      this.commit(label);
      $('resizeWidth').value = String(this.doc.width);
      $('resizeHeight').value = String(this.doc.height);
      this.setStatus(`${label} — image is now ${this.doc.width} x ${this.doc.height} px.`, 'ok');
    });
  }

  showResizeError(message) {
    $('resizeError').textContent = message ?? '';
  }

  async applyResize() {
    if (!this.doc || this.busy.busy || this.adjustments.active) return;
    const width = Number($('resizeWidth').value);
    const height = Number($('resizeHeight').value);
    const error = validateDimensions(width, height);
    if (error) {
      this.showResizeError(error);
      this.setStatus(`Resize rejected: ${error}`, 'error');
      return;
    }
    if (width === this.doc.width && height === this.doc.height) {
      this.showResizeError('The new size is identical to the current size.');
      return;
    }
    await this.busy.run('Resizing', async () => {
      const result = resizeDocument(this.doc, width, height);
      if (!result.ok) {
        this.showResizeError(result.error);
        this.setStatus(`Resize rejected: ${result.error}`, 'error');
        return;
      }
      this.showResizeError('');
      this.commit(`Resize to ${width} x ${height}`);
      if (this.crop.active) { this.crop.reset(this.doc); this.syncCropInputs(); }
      this.setStatus(`Resized to ${result.width} x ${result.height} px.`, 'ok');
    });
  }

  // ------------------------------------------------------------ adjustments
  readAdjustments() {
    return {
      brightness: Number($('brightness').value),
      contrast: Number($('contrast').value),
      grayscale: $('grayscaleToggle').checked,
      invert: $('invertToggle').checked,
    };
  }

  scheduleAdjustPreview(delay = 60) {
    if (!this.doc) return;
    clearTimeout(this.adjustTimer);
    this.adjustTimer = setTimeout(() => {
      this.adjustRun = this.refreshAdjustPreview();
    }, delay);
  }

  async refreshAdjustPreview() {
    if (!this.doc || this.busy.busy) return;
    const adjustments = this.readAdjustments();
    const wasActive = this.adjustments.active;
    const active = await this.busy.run('Previewing adjustments', async (progress) =>
      this.adjustments.update(this.doc, adjustments, progress));
    this.viewport.setLayers(this.currentLayers());
    this.viewport.requestRender();
    if (active !== wasActive) this.updateUi();
    if (!active) this.setStatus('Adjustments are at their neutral defaults; no preview shown.', 'info');
  }

  async applyAdjustments() {
    if (!this.doc || this.busy.busy) return;
    clearTimeout(this.adjustTimer);
    await this.adjustRun;
    if (!this.adjustments.active) {
      this.setStatus('Nothing to apply: adjustments are at their neutral defaults.', 'warn');
      return;
    }
    const adjustments = { ...this.adjustments.adjustments };
    await this.busy.run('Applying adjustments', async () => {
      this.adjustments.commit(this.doc);
      this.adjustments.clear();
      this.commit(describeAdjustments(adjustments));
      this.resetAdjustmentControls();
      this.setStatus(`Applied ${describeAdjustments(adjustments)}.`, 'ok');
    });
  }

  async cancelAdjustments() {
    if (!this.doc) return;
    clearTimeout(this.adjustTimer);
    await this.adjustRun;
    this.adjustments.clear();
    this.resetAdjustmentControls();
    this.viewport.setLayers(this.currentLayers());
    this.viewport.requestRender();
    this.updateUi();
    this.setStatus('Adjustment preview cancelled; the document is unchanged.', 'info');
  }

  resetAdjustmentControls() {
    $('brightness').value = '0';
    $('contrast').value = '0';
    $('brightnessValue').textContent = '0';
    $('contrastValue').textContent = '0';
    $('grayscaleToggle').checked = false;
    $('invertToggle').checked = false;
  }

  // ----------------------------------------------------------------- export
  readExportOptions() {
    const format = $('exportFormat').value === 'jpeg' ? 'jpeg' : 'png';
    return {
      format,
      quality: Number($('jpegQuality').value) / 100,
      background: $('exportBackground').value || EXPORT_BACKGROUND_DEFAULT,
      filename: $('exportFilename').value,
    };
  }

  showExportError(message) {
    $('exportError').textContent = message ?? '';
  }

  async runExport() {
    if (!this.doc || this.busy.busy) return;
    if (this.adjustments.active) {
      this.setStatus('Apply or cancel the adjustment preview before exporting.', 'warn');
      return;
    }
    const options = this.readExportOptions();
    const expectedWidth = this.doc.width;
    const expectedHeight = this.doc.height;
    try {
      const result = await this.busy.run('Exporting', async (progress) => {
        progress(0.15);
        const encoded = await exportDocument(this.doc, options);
        progress(0.7);
        const check = await verifyEncodedDimensions(encoded.blob, expectedWidth, expectedHeight);
        if (!check.ok) throw new Error(check.error);
        await nextFrame();
        downloadBlob(encoded.blob, encoded.filename);
        progress(1);
        return encoded;
      });
      this.showExportError('');
      this.lastExport = { blob: result.blob, filename: result.filename, type: result.type };
      $('reopenExportButton').disabled = false;
      this.dirty = false;
      this.updateUi();
      this.setStatus(
        `Exported ${result.filename} — ${result.width} x ${result.height} px, ${EXPORT_FORMATS[result.format].label}, ${formatBytes(result.blob.size)}.`,
        'ok',
      );
    } catch (error) {
      this.showExportError(error.message);
      this.setStatus(`Export failed: ${error.message}`, 'error');
    }
  }

  /** Load the most recent export back in as the current document. */
  async reopenLastExport() {
    if (!this.lastExport) return;
    const file = new File([this.lastExport.blob], this.lastExport.filename, { type: this.lastExport.type });
    await this.openFile(file);
  }

  // --------------------------------------------------------------- keyboard
  onKeyDown(event) {
    if (event.target instanceof HTMLInputElement || event.target instanceof HTMLSelectElement) {
      if (event.key === 'Escape') event.target.blur();
      return;
    }
    const ctrl = event.ctrlKey || event.metaKey;
    if (ctrl) {
      switch (event.key.toLowerCase()) {
        case 'z':
          event.preventDefault();
          if (event.shiftKey) this.redo(); else this.undo();
          return;
        case 'y':
          event.preventDefault();
          this.redo();
          return;
        case 'o':
          event.preventDefault();
          $('openInput').click();
          return;
        case 's':
          event.preventDefault();
          this.runExport();
          return;
        case '0':
          event.preventDefault();
          this.viewport.fit();
          return;
        case '1':
          event.preventDefault();
          this.viewport.actualSize();
          return;
        case '=':
        case '+':
          event.preventDefault();
          this.viewport.zoomStep(1);
          return;
        case '-':
          event.preventDefault();
          this.viewport.zoomStep(-1);
          return;
        default:
          return;
      }
    }
    if (event.key === ' ') {
      event.preventDefault();
      this.spaceHeld = true;
      this.updateCursorStyle();
      return;
    }
    if (event.key === 'Escape') {
      if (this.adjustments.active) this.cancelAdjustments();
      else if (this.tool === 'crop') this.cancelCrop();
      return;
    }
    if (event.key === 'Enter') {
      if (this.adjustments.active) this.applyAdjustments();
      else if (this.tool === 'crop') this.applyCrop();
      return;
    }
    if (event.key === '[' || event.key === ']') {
      const delta = event.key === '[' ? -1 : 1;
      this.setBrushSize(this.brushSize + delta * (event.shiftKey ? 10 : 1));
      return;
    }
    const tool = TOOL_SHORTCUTS[event.key.toLowerCase()];
    if (tool) {
      event.preventDefault();
      this.setTool(tool);
    }
  }

  setBrushSize(size) {
    const next = clamp(Math.round(size), BRUSH_SIZE_MIN, BRUSH_SIZE_MAX);
    this.brushSize = next;
    $('brushSize').value = String(next);
    $('brushSizeValue').textContent = `${next} px`;
    this.viewport.requestOverlayRender();
  }

  // ------------------------------------------------------------------ state
  setStatus(message, kind = 'info') {
    this.statusMessage = message;
    this.statusKind = kind;
    this.renderStatus();
  }

  renderStatus() {
    const el = $('statusText');
    if (this.busy.busy) {
      el.textContent = `${this.busy.label}…`;
      el.dataset.kind = 'info';
    } else {
      el.textContent = this.statusMessage;
      el.dataset.kind = this.statusKind;
    }
  }

  updateZoomReadout() {
    $('zoomLevel').textContent = `${(this.viewport.scale * 100).toFixed(this.viewport.scale < 1 ? 1 : 0)}%`;
    this.updateStatusBar();
  }

  updateStatusBar() {
    $('statusDimensions').textContent = this.doc ? `${this.doc.width} x ${this.doc.height}` : '—';
    $('statusZoom').textContent = this.doc
      ? `${(this.viewport.scale * 100).toFixed(this.viewport.scale < 1 ? 1 : 0)}%`
      : '—';
    $('statusHistory').textContent = this.history
      ? `${this.history.undoDepth} undo / ${this.history.redoDepth} redo${this.history.limitReached ? ' (limited)' : ''}`
      : '—';
    $('statusMemory').textContent = this.history
      ? `${formatBytes(this.history.memoryBytes)} of ${formatBytes(this.historyBudgetBytes)}`
      : '—';
  }

  updateUi() {
    const busy = this.busy.busy;
    const hasImage = this.hasImage;
    const pendingAdjust = this.adjustments.active;
    const locked = busy || pendingAdjust;
    const hasCropRect = this.crop.active;

    document.body.dataset.busy = String(busy);
    document.body.dataset.hasImage = String(hasImage);
    $('busyBar').hidden = !busy;

    $('openButton').disabled = busy;
    $('reopenExportButton').disabled = !this.lastExport || busy;
    $('exportButton').disabled = !hasImage || locked;
    $('exportRunButton').disabled = !hasImage || locked;
    $('undoButton').disabled = !hasImage || busy || !this.history?.canUndo;
    $('redoButton').disabled = !hasImage || busy || !this.history?.canRedo;
    $('undoButton').title = this.history?.nextUndoLabel ? `Undo ${this.history.nextUndoLabel} (Ctrl+Z)` : 'Undo (Ctrl+Z)';
    $('redoButton').title = this.history?.nextRedoLabel ? `Redo ${this.history.nextRedoLabel} (Ctrl+Y)` : 'Redo (Ctrl+Y)';

    for (const button of $('toolButtons').querySelectorAll('button[data-tool]')) {
      const tool = button.dataset.tool;
      button.setAttribute('aria-pressed', String(tool === this.tool));
      button.disabled = !hasImage || busy || (pendingAdjust && tool !== 'pan');
    }

    $('zoomInButton').disabled = !hasImage || this.viewport.scale >= ZOOM_MAX;
    $('zoomOutButton').disabled = !hasImage || this.viewport.scale <= ZOOM_MIN;
    $('zoomFitButton').disabled = !hasImage;
    $('zoomActualButton').disabled = !hasImage;

    for (const id of ['resizeWidth', 'resizeHeight', 'resizeLock', 'resizeApply',
      'rotateCw', 'rotateCcw', 'rotate180', 'mirrorH', 'mirrorV',
      'brightness', 'contrast', 'grayscaleToggle', 'invertToggle', 'adjustApply', 'adjustReset',
      'exportFormat', 'exportFilename', 'brushSize', 'brushColor', 'opacity', 'shapeMode',
      'aspectPreset', 'cropX', 'cropY', 'cropW', 'cropH', 'cropApply', 'cropCancel', 'cropSelectAll']) {
      $(id).disabled = !hasImage || locked;
    }
    $('adjustCancel').disabled = !hasImage || busy || !pendingAdjust;

    const isCrop = this.tool === 'crop';
    $('cropOptions').hidden = !isCrop;
    $('shapeOptions').hidden = !SHAPE_TOOLS.has(this.tool);
    $('pickerOptions').hidden = this.tool !== 'picker';
    $('paintOptions').hidden = !(PAINTING_TOOLS.has(this.tool));
    $('paintOptions').querySelector('h3').textContent = this.tool === 'eraser' ? 'Eraser' : 'Paint';
    $('brushColor').closest('.field').hidden = this.tool === 'eraser';

    $('cropApply').title = hasCropRect ? 'Apply the crop selection (Enter)' : 'Drag a crop rectangle first';
    this.updateCursorStyle();
    this.updateZoomReadout();
  }
}

function toolLabel(tool) {
  return { brush: 'Brush', eraser: 'Eraser', line: 'Line', rect: 'Rectangle', ellipse: 'Ellipse', picker: 'Eyedropper', crop: 'Crop', pan: 'Pan' }[tool] ?? tool;
}

function describeAdjustments(adjustments) {
  const parts = [];
  if (adjustments.brightness) parts.push(`brightness ${adjustments.brightness > 0 ? '+' : ''}${adjustments.brightness}`);
  if (adjustments.contrast) parts.push(`contrast ${adjustments.contrast > 0 ? '+' : ''}${adjustments.contrast}`);
  if (adjustments.grayscale) parts.push('grayscale');
  if (adjustments.invert) parts.push('invert');
  return parts.length ? `Adjustments (${parts.join(', ')})` : 'Adjustments';
}

function defaultExportName(sourceName) {
  const base = String(sourceName || 'image').replace(/\.[a-z0-9]+$/i, '');
  return `${base || 'image'}-edited`;
}

const editor = new Editor();
editor.init();
window.__rasterEditor = editor;
