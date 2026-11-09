// Jay-C's theme contrast test (frontend/test/theme-contrast.test.js there), self-contained: the WCAG
// math is inline instead of coming from Jay-C's core/appearance.js, so the same file runs in every Jay
// app that shares styles/theme.css. Changed a color? Run `npm test`.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { STYLES, ICON_COLORS } from '../src/core/style.js';

/** WCAG relative luminance of a #rrggbb color. */
function luminance(hex) {
  const [r, g, b] = [1, 3, 5].map((i) => {
    const c = parseInt(hex.slice(i, i + 2), 16) / 255;
    return c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

function contrastRatio(a, b) {
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return (hi + 0.05) / (lo + 0.05);
}

// The hex tokens of each block in theme.css, keyed by its selector.
const css = readFileSync(new URL('../src/styles/theme.css', import.meta.url), 'utf8');
const blocks = new Map();
for (const [, selector, body] of css.matchAll(/([^{}]+)\{([^{}]*)\}/g)) {
  const vars = Object.fromEntries([...body.matchAll(/(--[\w-]+):\s*(#[0-9a-f]{6})\s*;/gi)].map(([, k, v]) => [k, v.toLowerCase()]));
  for (const part of selector.replace(/\/\*[\s\S]*?\*\//g, '').split(',')) {
    const key = part.trim();
    blocks.set(key, { ...blocks.get(key), ...vars });
  }
}

/** Colors as the cascade resolves them for a style and theme (same order as theme.css). */
function palette(style, theme) {
  const layers = [':root'];
  if (theme === 'dark') layers.push(":root[data-theme='dark']");
  if (style !== 'calm') layers.push(`:root[data-style='${style}']`);
  if (style !== 'calm' && theme === 'dark') layers.push(`:root[data-style='${style}'][data-theme='dark']`);
  return Object.assign({}, ...layers.map((l) => blocks.get(l) ?? assert.fail(`no block ${l}`)));
}

test('the WCAG math matches known values', () => {
  assert.equal(contrastRatio('#000000', '#ffffff'), 21);
  assert.equal(contrastRatio('#777777', '#777777'), 1);
  assert.ok(Math.abs(contrastRatio('#767676', '#ffffff') - 4.54) < 0.01);
});

for (const style of STYLES) {
  for (const theme of ['light', 'dark']) {
    test(`${style} / ${theme}: text and controls are readable`, () => {
      const p = palette(style, theme);
      const check = (fg, bg, min) => {
        // A token this app doesn't define (e.g. Jay-C's --speaking) is skipped.
        if (!p[fg] || !p[bg]) return;
        const ratio = contrastRatio(p[fg], p[bg]);
        assert.ok(ratio >= min, `${fg} on ${bg}: ${ratio.toFixed(2)} < ${min}`);
      };
      for (const bg of ['--bg', '--surface', '--surface-2', '--surface-3']) {
        check('--text', bg, 7);
        check('--text-muted', bg, 4.5);
      }
      check('--accent-contrast', '--accent', 4.5); // button labels
      check('--accent', '--surface', 3); // accent as an icon or ring
      check('--danger', '--surface', 4.5); // error text
      if (p['--danger-fill']) assert.ok(contrastRatio('#ffffff', p['--danger-fill']) >= 4.5, 'white on --danger-fill');
      check('--speaking', '--surface', 3);
      check('--success', '--surface', 3);
    });
  }
}

test("the wolf icon's colors are each style's dark accent and accent contrast", () => {
  for (const style of STYLES) {
    const p = palette(style, 'dark');
    assert.deepEqual(ICON_COLORS[style], { lines: p['--accent'], fill: p['--accent-contrast'] }, style);
  }
});
