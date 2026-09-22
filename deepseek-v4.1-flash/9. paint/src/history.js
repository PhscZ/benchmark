import { HISTORY_BUDGET_BYTES, HISTORY_MIN_STATES } from './config.js';

/**
 * Bounded undo/redo stack of full-document snapshots.
 *
 * `states[index]` always equals the current document content; undo moves the
 * pointer back and reapplies that state (pixels *and* dimensions), redo moves it
 * forward. Committing after an undo truncates the redo branch.
 *
 * Memory is bounded by `budgetBytes`; when a commit would exceed it the oldest
 * states are evicted (never below `minStates`) and `onEvict` is notified so the
 * UI can show the visible "older history discarded" notice.
 */
export class History {
  /**
   * @param {import('./doc.js').RasterDocument} doc
   * @param {{budgetBytes?:number,minStates?:number,onEvict?:(info:{evicted:number,memoryBytes:number,states:number})=>void}} [options]
   */
  constructor(doc, options = {}) {
    this.doc = doc;
    this.budgetBytes = options.budgetBytes ?? HISTORY_BUDGET_BYTES;
    this.minStates = Math.max(1, options.minStates ?? HISTORY_MIN_STATES);
    this.onEvict = options.onEvict ?? null;
    /** @type {Array<{width:number,height:number,data:ImageData,label:string}>} */
    this.states = [];
    this.index = -1;
    this.bytes = 0;
    this.evictedTotal = 0;
    this.limitReached = false;
  }

  /** Discard all history and record the document's current content as the base state. */
  reset() {
    const snapshot = this.doc.snapshot();
    this.states = [{ ...snapshot, label: 'Opened image' }];
    this.index = 0;
    this.bytes = snapshot.data.data.length;
    this.evictedTotal = 0;
    this.limitReached = false;
  }

  /**
   * Record the document's current content as a new state.
   * @param {string} label human readable operation name
   */
  commit(label) {
    if (this.index < 0) { this.reset(); return; }
    if (this.index < this.states.length - 1) {
      for (let i = this.index + 1; i < this.states.length; i++) this.bytes -= this.states[i].data.data.length;
      this.states.length = this.index + 1;
    }
    const snapshot = this.doc.snapshot();
    this.states.push({ ...snapshot, label });
    this.bytes += snapshot.data.data.length;
    this.index = this.states.length - 1;
    this.evict();
  }

  evict() {
    let evicted = 0;
    while (this.bytes > this.budgetBytes && this.states.length > this.minStates && this.index > 0) {
      this.bytes -= this.states[0].data.data.length;
      this.states.shift();
      this.index -= 1;
      evicted += 1;
    }
    if (!evicted) return;
    this.evictedTotal += evicted;
    this.limitReached = true;
    this.onEvict?.({ evicted, memoryBytes: this.bytes, states: this.states.length });
  }

  get canUndo() { return this.index > 0; }
  get canRedo() { return this.index >= 0 && this.index < this.states.length - 1; }
  get undoDepth() { return Math.max(0, this.index); }
  get redoDepth() { return Math.max(0, this.states.length - 1 - this.index); }
  get stateCount() { return this.states.length; }
  get memoryBytes() { return this.bytes; }
  get nextUndoLabel() { return this.canUndo ? this.states[this.index].label : null; }
  get nextRedoLabel() { return this.canRedo ? this.states[this.index + 1].label : null; }

  /** @returns {boolean} whether the document changed */
  undo() {
    if (!this.canUndo) return false;
    this.index -= 1;
    this.apply();
    return true;
  }

  /** @returns {boolean} whether the document changed */
  redo() {
    if (!this.canRedo) return false;
    this.index += 1;
    this.apply();
    return true;
  }

  apply() {
    const state = this.states[this.index];
    this.doc.applySnapshot({ width: state.width, height: state.height, data: state.data });
  }
}
