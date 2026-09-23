import test from 'node:test';
import assert from 'node:assert/strict';
import { History } from '../../src/history.js';
import { HISTORY_BUDGET_BYTES } from '../../src/config.js';

/** Minimal stand-in for RasterDocument: snapshots are plain byte buffers. */
class FakeDoc {
  constructor(width, height) {
    this.width = width;
    this.height = height;
    this.fill = 0;
    this.applied = null;
  }

  snapshot() {
    const data = new Uint8ClampedArray(this.width * this.height * 4);
    data.fill(this.fill);
    return { width: this.width, height: this.height, data: { data } };
  }

  applySnapshot(snapshot) {
    this.width = snapshot.width;
    this.height = snapshot.height;
    this.applied = snapshot.data.data[0];
  }
}

test('supports at least 20 undoable operations at 1920x1080 within the documented budget', () => {
  const doc = new FakeDoc(1920, 1080);
  const history = new History(doc);
  history.reset();
  for (let i = 1; i <= 25; i++) {
    doc.fill = i;
    history.commit(`op ${i}`);
  }
  assert.ok(history.undoDepth >= 20, `expected >= 20 undo steps, got ${history.undoDepth}`);
  assert.ok(history.memoryBytes <= HISTORY_BUDGET_BYTES, 'memory must stay within the budget');
  assert.equal(history.limitReached, false, '1920x1080 must not need eviction');
});

test('evicts the oldest states past the budget and reports it', () => {
  const doc = new FakeDoc(4096, 4096);
  const events = [];
  const history = new History(doc, { onEvict: (info) => events.push(info) });
  history.reset();
  for (let i = 1; i <= 8; i++) {
    doc.fill = i;
    history.commit(`op ${i}`);
  }
  assert.ok(events.length > 0, 'eviction must be reported so the UI can show a notice');
  assert.equal(history.limitReached, true);
  assert.ok(history.memoryBytes <= HISTORY_BUDGET_BYTES);
  assert.ok(history.stateCount >= 2, 'the base state must be kept');
  assert.ok(history.canUndo, 'the most recent operation must stay undoable');
  assert.equal(events.at(-1).memoryBytes, history.memoryBytes);
});

test('undo restores pixel content and document dimensions', () => {
  const doc = new FakeDoc(20, 10);
  const history = new History(doc);
  history.reset();
  doc.fill = 7;
  history.commit('paint');
  doc.width = 8;
  doc.height = 4;
  doc.fill = 9;
  history.commit('crop');

  assert.equal(history.undo(), true);
  assert.equal(doc.width, 20);
  assert.equal(doc.height, 10);
  assert.equal(doc.applied, 7);

  assert.equal(history.redo(), true);
  assert.equal(doc.width, 8);
  assert.equal(doc.height, 4);
  assert.equal(doc.applied, 9);
});

test('committing after an undo discards the redo branch', () => {
  const doc = new FakeDoc(4, 4);
  const history = new History(doc);
  history.reset();
  doc.fill = 1; history.commit('a');
  doc.fill = 2; history.commit('b');
  assert.equal(history.undo(), true);
  assert.equal(history.canRedo, true);

  doc.fill = 3; history.commit('c');
  assert.equal(history.canRedo, false, 'the redo branch must be discarded');
  assert.equal(history.undoDepth, 2);
  assert.equal(history.nextUndoLabel, 'c');
});

test('undo and redo report labels for the UI and stop at the ends', () => {
  const doc = new FakeDoc(2, 2);
  const history = new History(doc);
  history.reset();
  doc.fill = 1; history.commit('Brush');
  assert.equal(history.nextUndoLabel, 'Brush');
  assert.equal(history.nextRedoLabel, null);
  history.undo();
  assert.equal(history.nextRedoLabel, 'Brush');
  assert.equal(history.canUndo, false);
  assert.equal(history.undo(), false);
  assert.equal(history.redo(), true);
  assert.equal(history.redo(), false);
});

test('reset drops every previous state', () => {
  const doc = new FakeDoc(64, 64);
  const history = new History(doc);
  history.reset();
  for (let i = 0; i < 5; i++) { doc.fill = i; history.commit(`op ${i}`); }
  const before = history.memoryBytes;
  history.reset();
  assert.equal(history.stateCount, 1);
  assert.equal(history.canUndo, false);
  assert.equal(history.canRedo, false);
  assert.ok(history.memoryBytes < before);
});
