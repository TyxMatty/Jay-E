import { test } from 'node:test';
import assert from 'node:assert/strict';
import { autoAdjust, curveAt, neutralAdjust } from '../src/core/image.js';

const flat = (from, to) => Array.from({ length: 256 }, (_, v) => (v >= from && v <= to ? 100 : 0));

test('auto levels stretch to the darkest and lightest pixels', () => {
  const hist = { r: flat(20, 200), g: flat(40, 220), b: flat(60, 240), l: flat(40, 220) };
  const tone = autoAdjust(hist, 'tone');
  assert.deepEqual(tone.levels.black.map((v) => Math.round(v * 255)), [20, 40, 60]);
  assert.deepEqual(tone.levels.white.map((v) => Math.round(v * 255)), [200, 220, 240]);
  // Contrast: the same for the three channels, so the colors stay.
  const contrast = autoAdjust(hist, 'contrast');
  assert.equal(new Set(contrast.levels.black).size, 1);
  assert.deepEqual([Math.round(contrast.levels.black[0] * 255), Math.round(contrast.levels.white[0] * 255)], [20, 240]);
  // Color: a blue-heavy picture gets warmer.
  const blue = { r: flat(0, 100), g: flat(0, 120), b: flat(100, 255), l: flat(0, 255) };
  assert.ok(autoAdjust(blue, 'color').temperature >= 0);
  assert.equal(autoAdjust({ r: flat(0, 0), g: flat(0, 0), b: flat(0, 0) }, 'tone').levels.white[0], 1, 'a one-value channel is left alone');
});

test('the curve goes through its points and never overshoots', () => {
  const pts = [[0, 0], [0.5, 0.75], [1, 1]];
  assert.equal(curveAt(pts, 0), 0);
  assert.equal(curveAt(pts, 0.5), 0.75);
  assert.equal(curveAt(pts, 1), 1);
  for (let i = 0; i < 100; i++) assert.ok(curveAt(pts, i / 100) <= curveAt(pts, (i + 1) / 100) + 1e-9);
  assert.equal(curveAt([], 0.3), 0.3);
  assert.equal(neutralAdjust().levels.gamma, 1);
});
