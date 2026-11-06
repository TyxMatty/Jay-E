import { test } from 'node:test';
import assert from 'node:assert/strict';
import { t, setLanguage, resolveLanguage } from '../src/core/i18n.js';
import ptBR from '../src/locales/pt-BR.js';

test('language preference: explicit, else from the system', () => {
  assert.equal(resolveLanguage('en', 'pt-BR'), 'en');
  assert.equal(resolveLanguage('pt-BR', 'en-US'), 'pt-BR');
  assert.equal(resolveLanguage('system', 'pt-BR'), 'pt-BR');
  assert.equal(resolveLanguage('system', 'pt-PT'), 'pt-BR');
  assert.equal(resolveLanguage('system', 'en-US'), 'en');
  assert.equal(resolveLanguage(undefined, 'de-DE'), 'en');
});

test('t() translates, falls back to English and fills placeholders', () => {
  setLanguage('en');
  assert.equal(t('Jay-E {version}', { version: '1.2' }), 'Jay-E 1.2');
  setLanguage('pt-BR');
  assert.equal(t("Couldn't open it: {error}", { error: 'access denied' }), 'Não deu pra abrir: access denied');
  assert.equal(t('a text nobody translated'), 'a text nobody translated');
  setLanguage('xx');
  assert.equal(t('Settings'), 'Settings');
});

test('every translation keeps the placeholders of its English text', () => {
  const names = (s) => [...s.matchAll(/\{(\w+)\}/g)].map((m) => m[1]).sort().join(',');
  for (const [en, pt] of Object.entries(ptBR)) {
    assert.equal(typeof pt, 'string', en);
    assert.ok(pt.trim(), `empty translation: ${en}`);
    assert.equal(names(pt), names(en), `placeholders differ: ${en}`);
  }
});

test('every literal t() text in the app has a pt-BR translation', async () => {
  const { readdir, readFile } = await import('node:fs/promises');
  const files = (await readdir(new URL('../src/', import.meta.url), { recursive: true })).filter((f) => f.endsWith('.js') && !f.includes('locales'));
  const missing = [];
  for (const file of files) {
    const code = await readFile(new URL(`../src/${file.replaceAll('\\', '/')}`, import.meta.url), 'utf8');
    // t('text') or t("text"), with escaped quotes inside.
    for (const [, , text] of code.matchAll(/\bt\((['"])((?:(?!\1)[^\\]|\\.)*)\1/g)) {
      const key = text.replace(/\\(.)/g, '$1');
      if (!(key in ptBR)) missing.push(`${file}: ${key}`);
    }
  }
  assert.deepEqual(missing, []);
});
