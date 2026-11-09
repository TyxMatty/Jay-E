// Find and replace in the document editor (stage 10). Matches are shown with the CSS Custom
// Highlight API, so the page itself isn't touched until something is replaced; replacing goes
// through the editor's own typing (execCommand), which keeps the formatting and marks the
// paragraph changed.
import { h } from './dom.js';
import { icons } from './icons.js';
import { t } from '../core/i18n.js';

/** The text nodes of `paragraphs` that are the document (not pictures or copies), in order. */
export function textNodes(paragraphs) {
  const out = [];
  for (const p of paragraphs) {
    const walker = document.createTreeWalker(p, NodeFilter.SHOW_TEXT, { acceptNode: (n) => (n.parentElement?.closest('[contenteditable="false"]') ? NodeFilter.FILTER_REJECT : NodeFilter.FILTER_ACCEPT) });
    const nodes = [];
    while (walker.nextNode()) nodes.push(walker.currentNode);
    out.push({ p, nodes });
  }
  return out;
}

/** Ranges of `query` in the paragraphs (a match may cross formatting, not paragraphs). */
export function findRanges(paragraphs, query, matchCase) {
  if (!query) return [];
  const needle = matchCase ? query : query.toLocaleLowerCase();
  const ranges = [];
  for (const { nodes } of textNodes(paragraphs)) {
    let text = '';
    const starts = [];
    for (const n of nodes) {
      starts.push(text.length);
      text += n.data;
    }
    const hay = matchCase ? text : text.toLocaleLowerCase();
    // Where offset `i` of the paragraph's text is: the node holding it and the offset in it.
    const at = (i, end) => {
      let k = starts.length - 1;
      while (k > 0 && (starts[k] > i || (end && starts[k] === i))) k--;
      return [nodes[k], i - starts[k]];
    };
    for (let i = hay.indexOf(needle); i !== -1 && ranges.length < 10000; i = hay.indexOf(needle, i + needle.length)) {
      const r = document.createRange();
      r.setStart(...at(i, false));
      r.setEnd(...at(i + needle.length, true));
      ranges.push(r);
    }
  }
  return ranges;
}

/**
 * The find bar.
 * @param {object} o
 * @param {() => HTMLElement[]} o.paragraphs the document's paragraphs
 * @param {(el: HTMLElement) => void} o.focusIn puts the caret in the box holding `el`
 * @param {() => void} o.checkpoint remembers the page for undo
 * @param {() => void} o.changed after a replacement
 */
export function createFindBar({ paragraphs, focusIn, checkpoint, changed }) {
  let ranges = [];
  let current = -1;
  /** How many the last "Replace all" replaced, shown until the search changes. */
  let replaced = 0;
  const findInput = h('input', { type: 'search', class: 'input', placeholder: t('Find'), 'aria-label': t('Find'), spellcheck: 'false' });
  const replaceInput = h('input', { type: 'text', class: 'input', placeholder: t('Replace with'), 'aria-label': t('Replace with'), spellcheck: 'false' });
  const caseBox = h('input', { type: 'checkbox', id: `dx-case-${Math.random().toString(36).slice(2)}` });
  const count = h('span', { class: 'hint dx-find-count', role: 'status' });
  const iconButton = (icon, label, onClick) => h('button', { type: 'button', class: 'icon-btn', title: label, 'aria-label': label, html: icons[icon], onClick });
  const bar = h(
    'div',
    { class: 'dx-find', role: 'search', hidden: true },
    h('span', { class: 'dx-find-icon', html: icons.search, 'aria-hidden': 'true' }),
    findInput,
    iconButton('up', t('Previous (Shift+Enter)'), () => go(-1)),
    iconButton('down', t('Next (Enter)'), () => go(1)),
    h('label', { class: 'dx-find-case', for: caseBox.id }, caseBox, t('Match case')),
    count,
    replaceInput,
    h('button', { type: 'button', class: 'btn btn-secondary', onClick: () => replaceOne() }, t('Replace')),
    h('button', { type: 'button', class: 'btn btn-secondary', onClick: () => replaceAll() }, t('Replace all')),
    iconButton('close', t('Close (Esc)'), () => close()),
  );

  function paint() {
    if (!globalThis.CSS?.highlights) return;
    CSS.highlights.set('jaye-find', new Highlight(...ranges));
    CSS.highlights.set('jaye-find-current', new Highlight(...(ranges[current] ? [ranges[current]] : [])));
  }
  function showCount() {
    count.textContent = !findInput.value ? '' : ranges.length ? t('{n} of {all}', { n: current + 1, all: ranges.length }) : replaced ? t('{n} replaced', { n: replaced }) : t('Not found');
  }
  /** Searches again; the current match is the first at or after `from` (a Range), if given. */
  function search(from) {
    ranges = findRanges(paragraphs(), findInput.value, caseBox.checked);
    current = ranges.length ? 0 : -1;
    if (from) {
      const k = ranges.findIndex((r) => r.compareBoundaryPoints(Range.START_TO_START, from) >= 0);
      current = k === -1 ? 0 : k;
    }
    paint();
    showCount();
  }
  function reveal() {
    const r = ranges[current];
    if (!r) return;
    r.startContainer.parentElement?.scrollIntoView({ block: 'center', behavior: 'smooth' });
  }
  function go(step) {
    if (!ranges.length) return;
    current = (current + step + ranges.length) % ranges.length;
    paint();
    showCount();
    reveal();
  }
  /** Types `text` over range `r` the way the person would. */
  function typeOver(r, text) {
    focusIn(r.startContainer.parentElement);
    const sel = getSelection();
    sel.removeAllRanges();
    sel.addRange(r);
    if (text) document.execCommand('insertText', false, text);
    else document.execCommand('delete');
  }
  function replaceOne() {
    const r = ranges[current];
    if (!r) return;
    checkpoint();
    typeOver(r, replaceInput.value);
    changed();
    const after = getSelection().rangeCount ? getSelection().getRangeAt(0) : null;
    search(after);
    reveal();
  }
  function replaceAll() {
    if (!ranges.length) return;
    checkpoint();
    const n = ranges.length;
    // From the last, so the earlier ranges stay where they are.
    for (const r of [...ranges].reverse()) typeOver(r, replaceInput.value);
    changed();
    replaced = n;
    search();
  }
  function open(withReplace = false) {
    bar.hidden = false;
    const sel = getSelection();
    const picked = sel.rangeCount && !sel.isCollapsed ? sel.toString() : '';
    if (picked && !picked.includes('\n') && picked.length < 200) findInput.value = picked;
    (withReplace && findInput.value ? replaceInput : findInput).focus();
    findInput.select();
    search(sel.rangeCount ? sel.getRangeAt(0) : null);
  }
  function close() {
    bar.hidden = true;
    ranges = [];
    current = -1;
    paint();
  }
  findInput.addEventListener('input', () => {
    replaced = 0;
    search();
  });
  caseBox.addEventListener('change', () => search());
  bar.addEventListener('keydown', (e) => {
    e.stopPropagation();
    if (e.key === 'Escape') close();
    else if (e.key === 'Enter' && e.target === findInput) {
      e.preventDefault();
      go(e.shiftKey ? -1 : 1);
    } else if (e.key === 'Enter' && e.target === replaceInput) {
      e.preventDefault();
      replaceOne();
    }
  });
  return { bar, open, close, refresh: () => !bar.hidden && search(ranges[current] ?? null) };
}
