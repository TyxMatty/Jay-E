// Spelling in the document editor (stage 10), in the file's own language with the Windows spell
// checker (src-tauri/src/spell.rs): the words on the page are sent, the wrong ones underlined with
// the CSS Custom Highlight API, and a right-click on one offers Windows' suggestions. Without a
// dictionary for that language, the browser's own spelling takes over.
import { h } from './dom.js';
import { t } from '../core/i18n.js';
import { textNodes } from './docx-find.js';

const call = async (cmd, args) => (await import('@tauri-apps/api/core')).invoke(cmd, args);
/** Words: letters (with their accents), maybe joined by an apostrophe or a hyphen. */
const WORD = /[\p{L}\p{M}]+(?:['’-][\p{L}\p{M}]+)*/gu;

/** The misspelled ranges of every open editor: one highlight for the whole window. */
const shown = new Map();
function paint() {
  if (globalThis.CSS?.highlights) CSS.highlights.set('jaye-spell', new Highlight(...[...shown.values()].flat()));
}

/** The words of some text, with where each starts. */
export function wordsIn(text) {
  return [...text.matchAll(WORD)].filter((m) => m[0].length > 1).map((m) => [m[0], m.index]);
}

/**
 * @param {object} o
 * @param {string} o.key this editor's id
 * @param {string} o.lang the language to check in ("pt-BR")
 * @param {() => HTMLElement[]} o.paragraphs
 * @param {HTMLElement[]} o.boxes the editable boxes (their `spellcheck` is turned off while this works)
 * @param {HTMLElement} o.page where the right-click is listened to
 * @param {() => void} o.checkpoint
 * @param {(lang: string | null) => void} o.onLang the language checked in, or null for none
 */
export function createSpelling({ key, lang, paragraphs, boxes, page, checkpoint, onLang }) {
  /** word → misspelled? */
  const known = new Map();
  let ranges = [];
  let working = true;
  let checkedIn = null;

  async function run() {
    if (!working) return;
    const found = [];
    for (const { nodes } of textNodes(paragraphs())) for (const node of nodes) for (const [word, at] of wordsIn(node.data)) found.push([node, word, at]);
    const unknown = [...new Set(found.map(([, w]) => w).filter((w) => !known.has(w)))];
    if (unknown.length) {
      let checked;
      try {
        checked = await call('spell_check', { lang, words: unknown });
      } catch {
        checked = null;
      }
      if (!checked) {
        // No dictionary for it: the browser's spelling, in the window's languages.
        working = false;
        for (const b of boxes) b.spellcheck = true;
        onLang(null);
        return;
      }
      if (checkedIn !== checked.lang) onLang((checkedIn = checked.lang));
      const wrong = new Set(checked.wrong);
      for (const w of unknown) known.set(w, wrong.has(w));
    }
    ranges = found
      .filter(([node, word]) => known.get(word) && node.isConnected)
      .map(([node, word, at]) => {
        const r = document.createRange();
        r.setStart(node, at);
        r.setEnd(node, at + word.length);
        return r;
      });
    shown.set(key, ranges);
    paint();
  }

  // shortcut: rereads every word after each pause in typing (only new words go to Windows); fine
  // for documents of a few hundred pages, look only at the changed paragraphs if that gets slow.
  let timer = 0;
  const schedule = () => {
    clearTimeout(timer);
    timer = setTimeout(run, 700);
  };

  // A right-click on an underlined word: Windows' suggestions.
  let menu = null;
  const closeMenu = () => {
    menu?.remove();
    menu = null;
  };
  page.addEventListener('contextmenu', async (e) => {
    if (!working) return;
    const at = document.caretRangeFromPoint?.(e.clientX, e.clientY);
    const r = at && ranges.find((q) => q.startContainer === at.startContainer && q.startOffset <= at.startOffset && at.startOffset <= q.endOffset);
    if (!r) return;
    e.preventDefault();
    closeMenu();
    const word = r.toString();
    const item = (label, onClick, extra = {}) =>
      h('button', { type: 'button', class: 'dx-spell-item', role: 'menuitem', onClick: () => (closeMenu(), onClick()), ...extra }, label);
    menu = h('div', { class: 'dx-spell-menu', role: 'menu', 'aria-label': t('Spelling') }, h('span', { class: 'hint' }, t('Looking…')));
    menu.style.cssText = `left:${e.clientX}px;top:${e.clientY}px`;
    document.body.append(menu);
    const own = menu;
    let suggestions = [];
    try {
      suggestions = await call('spell_suggest', { lang, word });
    } catch {}
    if (menu !== own) return;
    menu.replaceChildren(
      ...(suggestions.length ? suggestions.map((s) => item(s, () => fix(r, s), { class: 'dx-spell-item is-suggestion' })) : [h('span', { class: 'hint' }, t('No suggestions'))]),
      h('hr'),
      item(t('Ignore'), () => {
        known.set(word, false);
        run();
      }),
      item(t('Add to dictionary'), () => {
        known.set(word, false);
        call('spell_add', { lang, word }).catch(() => {});
        run();
      }),
    );
    // Kept inside the window.
    const box = menu.getBoundingClientRect();
    menu.style.left = `${Math.max(4, Math.min(e.clientX, innerWidth - box.width - 4))}px`;
    menu.style.top = `${Math.max(4, Math.min(e.clientY, innerHeight - box.height - 4))}px`;
    menu.querySelector('button')?.focus();
  });
  addEventListener('pointerdown', (e) => menu && !menu.contains(e.target) && closeMenu(), true);
  addEventListener('keydown', (e) => e.key === 'Escape' && closeMenu(), true);

  function fix(r, text) {
    checkpoint();
    const box = boxes.find((b) => b.contains(r.startContainer));
    box?.focus({ preventScroll: true });
    const sel = getSelection();
    sel.removeAllRanges();
    sel.addRange(r);
    document.execCommand('insertText', false, text);
    schedule();
  }

  for (const b of boxes) b.spellcheck = false;
  return {
    schedule,
    dispose() {
      clearTimeout(timer);
      closeMenu();
      shown.delete(key);
      paint();
    },
  };
}
