import { test } from 'node:test';
import assert from 'node:assert/strict';
import { formatSize, clock } from '../src/core/files.js';

test('sizes', () => {
  assert.equal(formatSize(0), '0 B');
  assert.equal(formatSize(1023), '1023 B');
  assert.equal(formatSize(1536), '1.5 KB');
  assert.equal(formatSize(20 * 1024 * 1024), '20.0 MB');
});

test('clock', () => {
  assert.equal(clock(0), '0:00.000');
  assert.equal(clock(62.5), '1:02.500');
  assert.equal(clock(59.9996), '1:00.000');
  assert.equal(clock(3600), '60:00.000');
  assert.equal(clock(NaN), '–');
  assert.equal(clock(-1), '–');
});
