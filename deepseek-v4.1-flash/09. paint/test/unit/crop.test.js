import test from 'node:test';
import assert from 'node:assert/strict';
import { CropSession, ASPECT_PRESETS } from '../../src/crop.js';

const doc = { width: 800, height: 600 };

function session(rect, aspect = null) {
  const crop = new CropSession();
  crop.rect = rect;
  crop.aspect = aspect;
  return crop;
}

test('validate accepts an in-bounds rectangle', () => {
  const result = session({ x: 10, y: 20, width: 100, height: 50 }).validate(doc);
  assert.equal(result.ok, true);
  assert.deepEqual(result.rect, { x: 10, y: 20, width: 100, height: 50 });
});

test('validate rejects out-of-bounds and degenerate rectangles with a reason', () => {
  assert.match(session({ x: -1, y: 0, width: 10, height: 10 }).validate(doc).error, /must be 0 or greater/);
  assert.match(session({ x: 750, y: 0, width: 100, height: 10 }).validate(doc).error, /exceeds the image width/);
  assert.match(session({ x: 0, y: 590, width: 10, height: 100 }).validate(doc).error, /exceeds the image height/);
  assert.match(session({ x: 0, y: 0, width: 0, height: 10 }).validate(doc).error, /at least 1 pixel/);
  assert.match(session(null).validate(doc).error, /Drag a crop rectangle/);
});

test('setNumeric accepts exact pixel values and reports field errors', () => {
  const crop = new CropSession();
  assert.deepEqual(crop.setNumeric(doc, { x: 5, y: 6, width: 7, height: 8 }), { ok: true, error: null });
  assert.deepEqual(crop.rect, { x: 5, y: 6, width: 7, height: 8 });

  assert.match(crop.setNumeric(doc, { x: 5.5, y: 6, width: 7, height: 8 }).error, /whole number/);
  assert.match(crop.setNumeric(doc, { x: 5, y: 6, width: 0, height: 8 }).error, /at least 1 pixel/);
  assert.match(crop.setNumeric(doc, { x: 0, y: 0, width: 900, height: 8 }).error, /exceeds the image width/);
});

test('setNumeric enforces a locked aspect ratio', () => {
  const crop = session(null, 1);
  assert.match(crop.setNumeric(doc, { x: 0, y: 0, width: 100, height: 50 }).error, /does not match the locked aspect ratio/);
  assert.equal(crop.setNumeric(doc, { x: 0, y: 0, width: 100, height: 100 }).ok, true);
});

test('hitTest finds the handle under the pointer and the interior for moves', () => {
  const crop = session({ x: 100, y: 100, width: 200, height: 100 });
  const tolerance = 9;
  assert.equal(crop.hitTest({ x: 100, y: 100 }, tolerance), 'nw');
  assert.equal(crop.hitTest({ x: 300, y: 100 }, tolerance), 'ne');
  assert.equal(crop.hitTest({ x: 100, y: 200 }, tolerance), 'sw');
  assert.equal(crop.hitTest({ x: 300, y: 200 }, tolerance), 'se');
  assert.equal(crop.hitTest({ x: 200, y: 100 }, tolerance), 'n');
  assert.equal(crop.hitTest({ x: 200, y: 200 }, tolerance), 's');
  assert.equal(crop.hitTest({ x: 100, y: 150 }, tolerance), 'w');
  assert.equal(crop.hitTest({ x: 300, y: 150 }, tolerance), 'e');
  assert.equal(crop.hitTest({ x: 200, y: 150 }, tolerance), 'move');
  assert.equal(crop.hitTest({ x: 600, y: 400 }, tolerance), 'new');
});

test('dragging the south-east handle keeps the north-west corner fixed', () => {
  const crop = session({ x: 100, y: 100, width: 200, height: 100 });
  crop.beginDrag('se', { x: 300, y: 200 }, doc);
  crop.drag({ x: 400, y: 300 }, doc, true);
  assert.deepEqual(crop.integerRect(), { x: 100, y: 100, width: 300, height: 200 });
});

test('dragging the north-west handle keeps the south-east corner fixed', () => {
  const crop = session({ x: 100, y: 100, width: 200, height: 100 });
  crop.beginDrag('nw', { x: 100, y: 100 }, doc);
  crop.drag({ x: 60, y: 40 }, doc, true);
  assert.deepEqual(crop.integerRect(), { x: 60, y: 40, width: 240, height: 160 });
});

test('a locked aspect ratio is preserved while dragging a corner', () => {
  const crop = session({ x: 100, y: 100, width: 200, height: 100 }, 16 / 9);
  crop.beginDrag('se', { x: 300, y: 200 }, doc);
  crop.drag({ x: 420, y: 260 }, doc, true);
  const rect = crop.integerRect();
  assert.ok(Math.abs(rect.width / rect.height - 16 / 9) < 0.02, `ratio was ${rect.width / rect.height}`);
});

test('a new drag creates a rectangle inside the document', () => {
  const crop = new CropSession();
  crop.beginDrag('new', { x: 700, y: 500 }, doc);
  crop.drag({ x: 850, y: 700 }, doc, false);
  const check = crop.validate(doc);
  assert.equal(check.ok, true, check.error);
  assert.ok(check.rect.x + check.rect.width <= doc.width);
  assert.ok(check.rect.y + check.rect.height <= doc.height);
});

test('moving cannot push the rectangle out of the document', () => {
  const crop = session({ x: 100, y: 100, width: 200, height: 100 });
  crop.beginDrag('move', { x: 200, y: 150 }, doc);
  crop.drag({ x: 5000, y: 5000 }, doc, true);
  const check = crop.validate(doc);
  assert.equal(check.ok, true, check.error);
  assert.equal(check.rect.x + check.rect.width, doc.width);
  assert.equal(check.rect.y + check.rect.height, doc.height);
});

test('aspect presets expose a freeform option and locked ratios', () => {
  const free = ASPECT_PRESETS.find((p) => p.id === 'free');
  assert.equal(free.ratio, null);
  assert.equal(ASPECT_PRESETS.find((p) => p.id === '16:9').ratio, 16 / 9);
  const crop = session({ x: 0, y: 0, width: 400, height: 400 });
  crop.setAspectPreset('1:1', doc);
  assert.ok(Math.abs(crop.rect.width - crop.rect.height) < 0.001);
});
