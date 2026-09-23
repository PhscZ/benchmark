import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { applyAdjustments, buildToneLut } from '../../src/adjust.js';

const fixture = JSON.parse(readFileSync(new URL('../fixtures/color_fixture.expected.json', import.meta.url), 'utf8'));

function rgbaFromFixture() {
  const pixelCount = fixture.width * fixture.height;
  const pixels = new Uint8ClampedArray(pixelCount * 4);
  for (let i = 0; i < pixelCount; i++) {
    pixels[i * 4] = fixture.input[i * 3];
    pixels[i * 4 + 1] = fixture.input[i * 3 + 1];
    pixels[i * 4 + 2] = fixture.input[i * 3 + 2];
    pixels[i * 4 + 3] = fixture.alpha[i];
  }
  return pixels;
}

for (const testCase of fixture.cases) {
  test(`matches the independent reference implementation: ${testCase.name}`, () => {
    const pixels = rgbaFromFixture();
    applyAdjustments(pixels, fixture.width, testCase.opts);
    const pixelCount = fixture.width * fixture.height;
    const actualRgb = [];
    for (let i = 0; i < pixelCount; i++) {
      actualRgb.push(pixels[i * 4], pixels[i * 4 + 1], pixels[i * 4 + 2]);
    }
    assert.deepEqual(actualRgb, testCase.expected, `${testCase.name} produced different colours than numpy`);
  });
}

test('alpha is never modified by any adjustment', () => {
  const pixels = rgbaFromFixture();
  const before = [];
  for (let i = 0; i < fixture.alpha.length; i++) before.push(pixels[i * 4 + 3]);
  applyAdjustments(pixels, fixture.width, { brightness: 60, contrast: -35, grayscale: true, invert: true });
  const after = [];
  for (let i = 0; i < fixture.alpha.length; i++) after.push(pixels[i * 4 + 3]);
  assert.deepEqual(after, before);
  assert.deepEqual(after, fixture.alpha);
});

test('neutral adjustments are the identity', () => {
  const pixels = rgbaFromFixture();
  const copy = Uint8ClampedArray.from(pixels);
  applyAdjustments(pixels, fixture.width, { brightness: 0, contrast: 0, grayscale: false, invert: false });
  assert.deepEqual(Array.from(pixels), Array.from(copy));
});

test('adjustments are row-chunkable without changing the result', () => {
  const whole = rgbaFromFixture();
  applyAdjustments(whole, fixture.width, { brightness: 25, contrast: 30, grayscale: true, invert: false });
  const chunked = rgbaFromFixture();
  const height = fixture.height;
  for (let y = 0; y < height; y += 2) {
    applyAdjustments(chunked, fixture.width, { brightness: 25, contrast: 30, grayscale: true, invert: false }, y, y + 2);
  }
  assert.deepEqual(Array.from(chunked), Array.from(whole));
});

test('brightness and contrast lookup table clamps at both ends', () => {
  const lut = buildToneLut(100, 0);
  assert.equal(lut[0], 255);
  assert.equal(lut[255], 255);
  const negative = buildToneLut(-100, 0);
  assert.equal(negative[0], 0);
  assert.equal(negative[255], 0);
  const neutral = buildToneLut(0, 0);
  for (let v = 0; v < 256; v++) assert.equal(neutral[v], v);
});
