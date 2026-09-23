import test from 'node:test';
import assert from 'node:assert/strict';
import { validateDimensions, lockAspect } from '../../src/transform.js';
import { MAX_DIMENSION } from '../../src/config.js';

test('validateDimensions accepts the supported range', () => {
  assert.equal(validateDimensions(1, 1), null);
  assert.equal(validateDimensions(4096, 4096), null);
  assert.equal(validateDimensions(1920, 1080), null);
});

test('validateDimensions rejects invalid sizes with a reason', () => {
  assert.match(validateDimensions(0, 100), /at least 1 pixel/);
  assert.match(validateDimensions(100, -5), /at least 1 pixel/);
  assert.match(validateDimensions(MAX_DIMENSION + 1, 100), /must not exceed 4096/);
  assert.match(validateDimensions(100.5, 100), /whole numbers/);
  assert.match(validateDimensions(Number.NaN, 100), /whole numbers/);
  assert.match(validateDimensions(Number.POSITIVE_INFINITY, 100), /whole numbers/);
});

test('lockAspect derives the other axis from the current document ratio', () => {
  assert.deepEqual(lockAspect(1920, 1080, 'width', 960), { width: 960, height: 540 });
  assert.deepEqual(lockAspect(1920, 1080, 'height', 540), { width: 960, height: 540 });
  assert.deepEqual(lockAspect(1920, 1080, 'width', 1920), { width: 1920, height: 1080 });
  assert.deepEqual(lockAspect(1000, 1000, 'width', 250), { width: 250, height: 250 });
  assert.deepEqual(lockAspect(1920, 1080, 'width', 1), { width: 1, height: 1 });
});
