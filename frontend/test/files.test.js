import { test } from 'node:test';
import assert from 'node:assert/strict';
import { decodeText, formatSize } from '../src/core/files.js';

test('text in UTF-8, or UTF-16 by its byte-order mark', () => {
  assert.equal(decodeText(new TextEncoder().encode('olá')), 'olá');
  assert.equal(decodeText(new Uint8Array([0xef, 0xbb, 0xbf, 0x68, 0x69])), 'hi');
  assert.equal(decodeText(new Uint8Array([0xff, 0xfe, 0x68, 0, 0x69, 0])), 'hi');
  assert.equal(decodeText(new Uint8Array([0xfe, 0xff, 0, 0x68, 0, 0x69])), 'hi');
});

test('sizes', () => {
  assert.equal(formatSize(0), '0 B');
  assert.equal(formatSize(1023), '1023 B');
  assert.equal(formatSize(1536), '1.5 KB');
  assert.equal(formatSize(20 * 1024 * 1024), '20.0 MB');
});
