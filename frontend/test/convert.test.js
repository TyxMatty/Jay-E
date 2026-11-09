import { test } from 'node:test';
import assert from 'node:assert/strict';
import { GROUPS, groupOf, outputOf, describeError } from '../src/core/convert.js';

test('every kind Jay-E converts has a group, others none', () => {
  assert.equal(groupOf({ kind: 'markdown' }).key, 'document');
  assert.equal(groupOf({ kind: 'image', error: 'unreadable' }), null);
  for (const kind of ['text', 'unknown']) assert.equal(groupOf({ kind }), null);
  assert.equal(groupOf({ kind: 'latex' }).key, 'latex');
  // An HTML page is read as a document; other text isn't converted.
  assert.equal(groupOf({ kind: 'text', name: 'Page.HTM' }).key, 'document');
  assert.equal(groupOf({ kind: 'text', name: 'notes.txt' }), null);
  // The same outputs as convert.rs can_become (image extensions, not format names).
  assert.deepEqual(GROUPS.find((g) => g.key === 'image').to.map(([to]) => to), ['webp', 'avif', 'jpg', 'png', 'gif', 'bmp', 'tif', 'ico', 'qoi']);
});

test('the chosen format, or the first', () => {
  assert.equal(outputOf({ kind: 'video' }, {}), 'mp4');
  assert.equal(outputOf({ kind: 'document' }, { document: 'html' }), 'html');
  assert.equal(outputOf({ kind: 'latex' }, {}), 'pdf');
  assert.equal(outputOf({ kind: 'pdf' }, { pdf: 'png' }), 'png');
  assert.equal(outputOf({ kind: 'text' }, {}), null);
});

test('errors in one line', () => {
  const t = (s) => s;
  assert.equal(describeError({ kind: 'invalid', message: 'bad header' }, t), 'bad header');
  assert.equal(describeError({ kind: 'password', message: '' }, t), 'The PDF has a password.');
  assert.equal(describeError({ kind: 'unsupported', what: 'VP9' }, t), 'VP9');
});
