import test from 'node:test';
import assert from 'node:assert/strict';
import { sanitizeFilename, ensureExtension, parseHexColor, rgbToHex, clamp } from '../../src/util.js';

test('sanitizeFilename removes path separators and control characters', () => {
  assert.equal(sanitizeFilename('../../etc/passwd', 'image'), '____etc_passwd');
  assert.equal(sanitizeFilename('my photo', 'image'), 'my photo');
  assert.equal(sanitizeFilename('a\\b:c*d?e"f<g>h|i', 'image'), 'a_b_c_d_e_f_g_h_i');
  assert.equal(sanitizeFilename('   ', 'image'), 'image');
  assert.equal(sanitizeFilename('', 'image'), 'image');
  assert.equal(sanitizeFilename(null, 'image'), 'image');
  assert.equal(sanitizeFilename('..', 'image'), 'image');
  assert.equal(sanitizeFilename('  .hidden.  ', 'image'), 'hidden');
});

test('ensureExtension appends the format extension only when missing', () => {
  assert.equal(ensureExtension('photo', 'png'), 'photo.png');
  assert.equal(ensureExtension('photo.png', 'png'), 'photo.png');
  assert.equal(ensureExtension('photo.PNG', 'png'), 'photo.PNG');
  assert.equal(ensureExtension('photo.jpg', 'png'), 'photo.jpg.png');
});

test('parseHexColor accepts 6-digit hex with or without the hash', () => {
  assert.deepEqual(parseHexColor('#ffffff'), { r: 255, g: 255, b: 255 });
  assert.deepEqual(parseHexColor('000000'), { r: 0, g: 0, b: 0 });
  assert.deepEqual(parseHexColor('#1a2b3c'), { r: 26, g: 43, b: 60 });
  assert.equal(parseHexColor('red'), null);
  assert.equal(parseHexColor('#fff'), null);
  assert.equal(parseHexColor(''), null);
});

test('rgbToHex clamps and pads each channel', () => {
  assert.equal(rgbToHex(0, 0, 0), '#000000');
  assert.equal(rgbToHex(255, 255, 255), '#ffffff');
  assert.equal(rgbToHex(300, -5, 16), '#ff0010');
  assert.equal(rgbToHex(10.6, 20.4, 30.5), '#0b141f');
});

test('clamp bounds values', () => {
  assert.equal(clamp(5, 0, 10), 5);
  assert.equal(clamp(-1, 0, 10), 0);
  assert.equal(clamp(11, 0, 10), 10);
});
