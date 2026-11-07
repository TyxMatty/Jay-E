import { test } from 'node:test';
import assert from 'node:assert/strict';
import { styleClass, defaultLevels, formatNumber, listMarkers, toHex, firstFamily, normalProps, sameProps, runSet } from '../src/core/docx.js';

test('style classes match the ones Rust writes', () => {
  assert.equal(styleClass('Heading1'), 'Heading1');
  assert.equal(styleClass('Título 1'), 'T_tulo_1');
  assert.equal(styleClass(''), '_default');
});

test('numbers in Word formats', () => {
  assert.equal(formatNumber(3, 'decimal'), '3');
  assert.equal(formatNumber(1, 'lowerLetter'), 'a');
  assert.equal(formatNumber(28, 'upperLetter'), 'BB');
  assert.equal(formatNumber(14, 'lowerRoman'), 'xiv');
  assert.equal(formatNumber(1990, 'upperRoman'), 'MCMXC');
  assert.equal(formatNumber(7, 'decimalZero'), '07');
});

test('list markers count through the document and restart deeper levels', () => {
  const levels = { 1: defaultLevels(false), 2: defaultLevels(true) };
  const nums = ['1:0', '1:1', '1:1', '', '1:0', '1:1', '2:0', '2:1', 'x'];
  assert.deepEqual(listMarkers(nums, (id) => levels[id] ?? defaultLevels(true)), ['1.', 'a.', 'b.', '', '2.', 'a.', '•', '◦', '•']);
  // Multi-level text like "1.2."
  const legal = defaultLevels(false).map((l, i) => ({ ...l, fmt: 'decimal', text: i ? '%1.%2.' : '%1.' }));
  assert.deepEqual(listMarkers(['1:0', '1:1', '1:1'], () => legal), ['1.', '1.1.', '1.2.']);
});

test('colors and fonts from computed CSS', () => {
  assert.equal(toHex('rgb(255, 0, 16)'), 'ff0010');
  assert.equal(toHex('rgba(0, 0, 0, 0)'), '');
  assert.equal(toHex('transparent'), '');
  assert.equal(firstFamily('"Times New Roman", serif'), 'Times New Roman');
  assert.equal(firstFamily('Calibri, sans-serif'), 'Calibri');
});

test('the run look against its styles', () => {
  const base = normalProps({ font: 'Aptos', size: 12, color: 'auto' }, { bold: true });
  assert.deepEqual(base, { font: 'Aptos', size: 12, bold: true, italic: false, underline: false, strike: false, color: '000000', bg: '', vert: '' });
  const same = { ...base };
  assert.ok(sameProps(same, { ...base, size: 12.2, font: 'aptos' }));
  assert.ok(!sameProps(same, { ...base, bold: false }));
  // Unchanged: nothing written; un-bolded where the style bolds: an explicit "not bold".
  assert.deepEqual(Object.values(runSet(same, base)).filter((v) => v !== null), []);
  const set = runSet({ ...base, bold: false, color: 'ff0000', font: 'Arial', size: 18 }, base);
  assert.deepEqual(set, { font: 'Arial', size: 18, bold: false, italic: null, underline: null, strike: null, color: 'ff0000', bg: null, vert: null });
});
