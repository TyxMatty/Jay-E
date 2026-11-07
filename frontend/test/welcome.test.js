import { test } from 'node:test';
import assert from 'node:assert/strict';
import { WELCOME_STEPS, clampStep, shouldShowWelcome } from '../src/core/welcome.js';
import { WELCOME_ART } from '../src/ui/welcome-art.js';
import ptBR from '../src/locales/pt-BR.js';

test('every step has a unique key, a title, a body, its art and a known action', () => {
  const keys = WELCOME_STEPS.map((s) => s.key);
  assert.equal(new Set(keys).size, keys.length);
  for (const step of WELCOME_STEPS) {
    assert.ok(step.title && step.body, step.key);
    assert.match(WELCOME_ART[step.art] ?? '', /^<svg/, step.key);
    if (step.action) assert.equal(step.action.target, 'settings', step.key);
  }
  assert.equal(WELCOME_STEPS[0].key, 'welcome');
});

// The steps go through t(step.title), which test/i18n.test.js can't see.
test('every step is translated', () => {
  for (const step of WELCOME_STEPS) {
    for (const text of [step.title, step.body, step.action?.label].filter(Boolean)) assert.ok(text in ptBR, text);
  }
});

test('clampStep keeps the index inside the tour', () => {
  const last = WELCOME_STEPS.length - 1;
  assert.equal(clampStep(-1), 0);
  assert.equal(clampStep(2), 2);
  assert.equal(clampStep(99), last);
  assert.equal(clampStep(1.7), 1);
  assert.equal(clampStep(NaN), 0);
});

test('shows until seen', () => {
  assert.equal(shouldShowWelcome({}), true);
  assert.equal(shouldShowWelcome({ welcomeSeen: false }), true);
  assert.equal(shouldShowWelcome({ welcomeSeen: true }), false);
  assert.equal(shouldShowWelcome(undefined), true);
});
