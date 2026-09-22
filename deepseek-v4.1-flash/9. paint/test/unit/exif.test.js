import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { readImageMeta, stripOrientation, sniffFormat } from '../../src/exif.js';

const fixture = (name) => new Uint8Array(readFileSync(new URL(`../fixtures/${name}`, import.meta.url)));

const ORIENTATIONS = [1, 3, 6, 8];

for (const extension of ['jpg', 'png', 'webp']) {
  for (const orientation of ORIENTATIONS) {
    test(`reads EXIF orientation ${orientation} from .${extension}`, () => {
      const meta = readImageMeta(fixture(`orient${orientation}.${extension}`));
      assert.ok(meta, 'container must be recognised');
      assert.equal(meta.orientation, orientation);
      assert.equal(meta.width, 8);
      assert.equal(meta.height, 4);
    });

    test(`stripping orientation from .${extension} keeps the container valid`, () => {
      const bytes = fixture(`orient${orientation}.${extension}`);
      const stripped = stripOrientation(bytes);
      const meta = readImageMeta(stripped);
      assert.ok(meta, 'stripped container must still parse');
      assert.equal(meta.orientation, 1, 'orientation must be gone so decoders cannot apply it');
      assert.equal(meta.width, 8, 'dimensions must be preserved');
      assert.equal(meta.height, 4);
      assert.ok(stripped.length <= bytes.length, 'stripping must not grow the file');
    });
  }
}

test('unsupported containers are rejected', () => {
  assert.equal(sniffFormat(fixture('unsupported.bmp')), null);
  assert.equal(readImageMeta(fixture('unsupported.bmp')), null);
  assert.equal(stripOrientation(fixture('unsupported.bmp')).length, fixture('unsupported.bmp').length);
});

test('truncated JPEG data does not throw', () => {
  const bytes = fixture('corrupt.jpg');
  assert.equal(sniffFormat(bytes), 'jpeg');
  const meta = readImageMeta(bytes);
  assert.ok(meta === null || typeof meta.orientation === 'number');
  assert.doesNotThrow(() => stripOrientation(bytes));
});

test('short and empty inputs do not throw', () => {
  assert.equal(sniffFormat(new Uint8Array()), null);
  assert.equal(readImageMeta(new Uint8Array([0xff, 0xd8])), null);
  assert.equal(stripOrientation(new Uint8Array([0xff, 0xd8])).length, 2);
});
