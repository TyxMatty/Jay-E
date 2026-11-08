import { test } from 'node:test';
import assert from 'node:assert/strict';
import { normalizeMotion, resolveMotion } from '../src/core/motion.js';

test('animations are on unless turned off', () => {
  assert.equal(resolveMotion(undefined), 'full');
  assert.equal(resolveMotion('on'), 'full');
  assert.equal(resolveMotion('system'), 'full');
  assert.equal(resolveMotion('off'), 'reduced');
  assert.equal(normalizeMotion('whatever'), 'on');
});
