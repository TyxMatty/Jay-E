import { test } from 'node:test';
import assert from 'node:assert/strict';
import { formatSize } from '../src/core/files.js';

test('sizes', () => {
  assert.equal(formatSize(0), '0 B');
  assert.equal(formatSize(1023), '1023 B');
  assert.equal(formatSize(1536), '1.5 KB');
  assert.equal(formatSize(20 * 1024 * 1024), '20.0 MB');
});
