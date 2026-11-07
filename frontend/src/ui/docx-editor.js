// The DOCX editor (stage 7): the document on its own page size, margins, fonts and styles, edited
// in place with a toolbar like the usual word processors. Rust (jay_docs::doc::edit) builds the page
// and, on save, rewrites only the paragraphs that changed; the rest of the file is copied as it was.
//
// Each paragraph, table, row and cell carries `data-u` (where it is in the file); each piece of text
// `data-r` (the run its properties come from); images, fields and other things shown but not edited
// are `.dx-atom[data-a]`. Paragraph settings live in data attributes (data-style, data-num,
// data-align, data-ind, data-line) that the toolbar changes and the save sends back.
import { h } from './dom.js';
import { icons } from './icons.js';
import { t } from '../core/i18n.js';
import { openLink } from '../core/pdf.js';
import { styleClass, defaultLevels, listMarkers, toHex, firstFamily, normalProps, sameProps, runSet } from '../core/docx.js';

const call = async (cmd, args) => (await import('@tauri-apps/api/core')).invoke(cmd, args);
const ZOOMS = [0.5, 0.75, 0.9, 1, 1.25, 1.5, 2];
const FONTS = ['Aptos', 'Arial', 'Calibri', 'Cambria', 'Comic Sans MS', 'Consolas', 'Courier New', 'Garamond', 'Georgia', 'Segoe UI', 'Tahoma', 'Times New Roman', 'Trebuchet MS', 'Verdana'];
const SPACINGS = [240, 276, 360, 480];
const ALIGNS = [
  ['left', 'alignLeft', 'Align left'],
  ['center', 'alignCenter', 'Center'],
  ['right', 'alignRight', 'Align right'],
  ['both', 'alignJustify', 'Justify'],
];
const WATCH = { subtree: true, childList: true, characterData: true, attributes: true, attributeFilter: ['data-style', 'data-num', 'data-align', 'data-ind', 'data-line', 'style', 'class', 'href'] };
let nextDoc = 0;

/** The name a style shows in the menu (Word's built-in names are in English inside the file). */
function styleName(name) {
  const lower = name.toLowerCase();
  const heading = /^heading (\d)$/.exec(lower);
  if (heading) return t('Heading {n}', { n: heading[1] });
  return { normal: t('Normal text'), standard: t('Normal text'), 'default paragraph style': t('Normal text'), 'text body': t('Body text'), 'body text': t('Body text'), quotations: t('Quote'), title: t('Title'), subtitle: t('Subtitle'), quote: t('Quote'), 'list paragraph': t('List paragraph') }[lower] ?? name;
}

export async function createDocxEditor(file, fallback) {
  let opened;
  try {
    opened = await call('docx_edit_open', { path: file.path });
  } catch (error) {
    // Files Jay-E reads but can't edit safely still open, read-only.
    if (error?.kind === 'invalid') return fallback();
    const message = error?.kind === 'toobig' ? t('This document is too big.') : (error?.message ?? String(error));
    return h('div', { class: 'view-notice' }, h('h2', {}, file.name), h('p', {}, t("Couldn't open it: {error}", { error: message })));
  }
  const id = opened.id;
  let view = opened.view;
  const docKey = String(++nextDoc);

  const styleEl = h('style');
  const body = h('div', { class: 'dx-body', contenteditable: 'true', spellcheck: 'true', role: 'textbox', 'aria-multiline': 'true', 'aria-label': file.name });
  // The default header and footer, edited in place on the first page and copied onto the others.
  const header = h('div', { class: 'dx-margin is-header', contenteditable: 'true', spellcheck: 'true', role: 'textbox', 'aria-multiline': 'true', 'aria-label': t('Header'), hidden: true });
  const footer = h('div', { class: 'dx-margin is-footer', contenteditable: 'true', spellcheck: 'true', role: 'textbox', 'aria-multiline': 'true', 'aria-label': t('Footer'), hidden: true });
  const copies = h('div', { class: 'dx-copies', 'aria-hidden': 'true' });
  const page = h('div', { class: 'dx-page is-paged', 'data-doc': docKey }, header, body, footer, copies);
  /** The places text is edited: the body, and the header and footer when the file has them. */
  const editables = () => [body, header, footer].filter((e) => !e.hidden);
  const allIn = (selector) => editables().flatMap((e) => [...e.querySelectorAll(selector)]);
  const inside = (node) => !!node && editables().some((e) => e.contains(node));
  const stage = h('div', { class: 'doc-stage dx-stage' }, page);

  // ----- What changed -----
  /** `data-u` of the paragraphs changed since the last save (new ones are always sent). */
  const dirty = new Set();
  let changed = false;
  /** The look of each original run when the page was built, by `data-r`. */
  let original = new Map();
  const observer = new MutationObserver((records) => note(records));
  function mark(node) {
    const el = node?.nodeType === 1 ? node : node?.parentElement;
    const p = el?.closest?.('p');
    if (p && inside(p) && p.dataset.u != null) dirty.add(p.dataset.u);
  }
  function note(records) {
    for (const r of records) {
      mark(r.target);
      r.addedNodes.forEach(mark);
    }
    if (records.length && !changed) {
      changed = true;
      showDirty();
    }
    if (records.length) scheduleRenumber();
  }

  // ----- Lists -----
  const levelsOf = (numId) => view.numbering[numId] ?? defaultLevels(numId !== 'new-number');
  const levelOf = (num) => {
    const [numId, lvl] = num.split(':');
    return levelsOf(numId)[Math.min(Number(lvl) || 0, 8)];
  };
  let renumberQueued = false;
  function scheduleRenumber() {
    if (renumberQueued) return;
    renumberQueued = true;
    requestAnimationFrame(renumber);
  }
  /** Draws the bullets and numbers (attributes the save doesn't look at). */
  function renumber() {
    renumberQueued = false;
    note(observer.takeRecords());
    const paras = allIn('p');
    const markers = listMarkers(
      paras.map((p) => p.dataset.num),
      levelsOf,
    );
    paras.forEach((p, i) => {
      if (markers[i] || p.dataset.num) {
        if (p.dataset.marker !== markers[i]) p.dataset.marker = markers[i];
        const hang = `${levelOf(p.dataset.num).hanging / 20}pt`;
        if (p.style.getPropertyValue('--hang') !== hang) p.style.setProperty('--hang', hang);
      } else if (p.hasAttribute('data-marker')) {
        p.removeAttribute('data-marker');
      }
    });
    paginate();
    // Floating pictures follow their paragraph as the text above them changes.
    layoutFloats();
    observer.takeRecords();
  }

  // ----- Pages: the body broken into the file's pages, with its header and footer on each -----
  const GAP = 12; // points between pages
  /**
   * A paragraph or table that would cross the bottom margin starts on the next page (pushed down
   * with a margin the save doesn't look at), as does what follows a page break. Tables longer than
   * a page aren't split.
   */
  function paginate() {
    // Not laid out yet (a tab in the background): nothing to measure.
    if (!page.isConnected || !page.offsetWidth) return;
    const pg = view.page;
    const px = 4 / 3;
    const height = pg.h * px;
    const stride = (pg.h + GAP) * px;
    const top = pg.top * px;
    const bottom = pg.bottom * px;
    const blocks = [...body.children].filter((el) => el.matches('p, table, .dx-block'));
    for (const el of blocks) {
      if (el.dataset.push) {
        el.style.marginTop = '';
        delete el.dataset.push;
      }
    }
    const rects = blocks.map((el) => rectIn(el));
    let offset = 0;
    let breakAfter = false;
    let end = top;
    blocks.forEach((el, i) => {
      const r = rects[i];
      let y = r.y + offset;
      const k = Math.floor(y / stride);
      let target = null;
      if (breakAfter) target = (k + 1) * stride + top;
      else if (y < k * stride + top - 0.5) target = k * stride + top;
      else if (y + r.h > k * stride + height - bottom + 0.5 && r.h <= height - top - bottom) target = (k + 1) * stride + top;
      if (target != null && target > y) {
        el.style.marginTop = `${target - y}px`;
        el.dataset.push = '1';
        offset += target - y;
        y = target;
      }
      end = y + r.h;
      breakAfter = !!el.querySelector('.dx-pagebreak');
    });
    const pages = Math.max(1, Math.floor((end + bottom - 1) / stride) + 1);
    page.style.minHeight = `${pages * stride - GAP * px - top - bottom}px`;
    page.style.setProperty('--dx-h', `${height}px`);
    page.style.setProperty('--dx-stride', `${stride}px`);
    // The header and footer on every page after the first, as pictures of the real ones.
    copies.replaceChildren();
    for (let k = 1; k < pages; k++) {
      for (const [el, at] of [
        [header, pg.header],
        [footer, pg.h - pg.footer],
      ]) {
        if (el.hidden) continue;
        const copy = el.cloneNode(true);
        copy.removeAttribute('contenteditable');
        copy.removeAttribute('data-root');
        copy.removeAttribute('role');
        copy.classList.add('is-copy');
        copy.style.top = `${k * (pg.h + GAP) + at}pt`;
        copies.append(copy);
      }
    }
  }

  // ----- The look of text -----
  /** What some text looks like on the page, the way the save compares it. */
  function propsOf(el, p) {
    const cs = getComputedStyle(el);
    let underline = false;
    let strike = false;
    let bg = '';
    let vert = '';
    let sizeEl = el;
    for (let e = el; e; e = e.parentElement) {
      const c = e === el ? cs : getComputedStyle(e);
      if (c.textDecorationLine.includes('underline')) underline = true;
      if (c.textDecorationLine.includes('line-through')) strike = true;
      if (!bg && e !== p) bg = toHex(c.backgroundColor);
      if (c.verticalAlign === 'super' || c.verticalAlign === 'sub') {
        vert ||= c.verticalAlign === 'super' ? 'superscript' : 'subscript';
        sizeEl = e.parentElement ?? e; // the size before "smaller"
      }
      if (e === p) break;
    }
    const size = parseFloat(getComputedStyle(sizeEl).fontSize) * 0.75;
    return {
      font: firstFamily(cs.fontFamily),
      size: Math.round(size * 2) / 2,
      bold: Number(cs.fontWeight) >= 600,
      italic: cs.fontStyle === 'italic',
      underline,
      strike,
      color: toHex(cs.color) || '000000',
      bg,
      vert,
    };
  }
  const styleOf = (p) => view.styles.find((s) => s.id === (p.dataset.style || view.default_style));
  const baseOf = (p, charStyle) => normalProps(styleOf(p)?.run, charStyle ? view.char_styles[charStyle] : null);

  function snapshot() {
    original = new Map();
    for (const span of allIn('[data-r]')) {
      const r = Number(span.dataset.r);
      if (!original.has(r) && span.firstChild) original.set(r, propsOf(span, span.closest('p')));
    }
  }

  // ----- Saving -----
  /** Text outside a paragraph (the browser sometimes leaves some) goes into one. */
  function normalize() {
    for (const box of [...editables(), ...allIn('td')]) {
      for (const n of [...box.childNodes]) {
        if (n.nodeType === 1 && n.matches('p, table, .dx-block')) continue;
        if (n.nodeType === 3 && !n.data) continue;
        const p = document.createElement('p');
        if (n.nodeType === 1 && n.tagName === 'DIV') {
          p.append(...n.childNodes);
          n.replaceWith(p);
        } else {
          n.replaceWith(p);
          p.append(n);
        }
      }
    }
  }

  function itemsOf(p) {
    const items = [];
    const push = (into, node) => {
      const el = node.parentElement;
      const run = el.closest('[data-r]');
      const r = run && p.contains(run) ? Number(run.dataset.r) : null;
      const cur = propsOf(el, p);
      const set = r != null && sameProps(cur, original.get(r)) ? null : runSet(cur, baseOf(p, run?.dataset.cs));
      const last = into.at(-1);
      if (last?.t === 'text' && last.r === r && JSON.stringify(last.set) === JSON.stringify(set)) last.text += node.data;
      else into.push({ t: 'text', text: node.data, r, set });
    };
    const walk = (node, into, inLink) => {
      for (const n of node.childNodes) {
        if (n.nodeType === 3) {
          if (n.data) push(into, n);
        } else if (n.nodeType === 1) {
          if (n.matches('.dx-atom[data-new]')) {
            const d = n.dataset;
            into.push({ t: 'image', n: Number(d.new), geo: { moved: !!d.moved, x: Number(d.hx) || 0, y: Number(d.vy) || 0, w: Number(d.w), h: Number(d.h) } });
          } else if (n.matches('.dx-atom[data-a]')) {
            const d = n.dataset;
            const geo = d.geo ? { moved: !!d.moved, x: Number(d.hx) || 0, y: Number(d.vy) || 0, w: Number(d.w), h: Number(d.h) } : null;
            into.push({ t: 'atom', a: Number(d.a), geo });
          }
          else if (n.tagName === 'BR') into.push({ t: 'br' });
          else if (n.tagName === 'A' && !inLink) {
            const inner = [];
            walk(n, inner, true);
            into.push({ t: 'link', href: n.getAttribute('href') ?? '', h: n.dataset.h != null ? Number(n.dataset.h) : null, items: inner });
          } else walk(n, into, inLink);
        }
      }
    };
    walk(p, items, false);
    // The browser's placeholder line in an empty (or Shift+Enter-ended) paragraph.
    if (items.at(-1)?.t === 'br') items.pop();
    return items;
  }

  function collect() {
    note(observer.takeRecords());
    normalize();
    note(observer.takeRecords());
    const paras = allIn('p');
    // Enter copies a paragraph's attributes: the second one with the same data-u is new.
    const seen = new Set();
    const fresh = new Set();
    for (const p of paras) {
      if (p.dataset.u == null || seen.has(p.dataset.u)) fresh.add(p);
      else seen.add(p.dataset.u);
    }
    const present = [...new Set(allIn('[data-u]').filter((e) => !fresh.has(e)).map((e) => Number(e.dataset.u)))];
    const out = [];
    const tables = [];
    let order = 0;
    /** The unit a new thing goes after: the closest earlier sibling that is in the file. */
    const afterOf = (el) => {
      for (let s = el.previousElementSibling; s; s = s.previousElementSibling) if (s.dataset.u != null && !fresh.has(s)) return Number(s.dataset.u);
      return null;
    };
    const containerOf = (el) => {
      const box = el.parentElement?.closest('td[data-u], [data-root], .dx-body');
      return box?.matches('td') ? Number(box.dataset.u) : box?.dataset.root != null ? Number(box.dataset.root) : null;
    };
    const newPara = (p) => ({ order: 0, u: null, base: null, after: null, container: null, style: p.dataset.style ?? '', num: p.dataset.num ?? '', align: p.dataset.align ?? '', ind: p.dataset.ind ?? '', line: p.dataset.line ?? '', items: itemsOf(p) });
    // New tables go whole; their paragraphs aren't sent again below.
    for (const table of allIn('table:not([data-u])')) {
      if (table.parentElement.closest('table:not([data-u])')) continue;
      const rows = [...table.rows].map((tr) => [...tr.cells].map((td) => [...td.children].filter((c) => c.tagName === 'P').map(newPara)));
      const cols = [...(table.rows[0]?.cells ?? [])].map((td) => (parseFloat(td.style.width) || 0) || null).filter(Boolean);
      tables.push({ el: table, after: afterOf(table), container: containerOf(table), cols, rows });
    }
    const inNewTable = (el) => !!el.closest('table:not([data-u])');
    const blocks = allIn('p, table');
    for (const t of tables) t.order = blocks.indexOf(t.el) + 1;
    for (const t of tables) delete t.el;
    for (const p of paras) {
      order = blocks.indexOf(p) + 1;
      const isNew = fresh.has(p);
      if (inNewTable(p) || (!isNew && !dirty.has(p.dataset.u))) continue;
      const after = afterOf(p);
      const cell = p.parentElement?.closest('td[data-u], [data-root], .dx-body');
      out.push({
        order,
        u: isNew ? null : Number(p.dataset.u),
        base: p.dataset.u != null ? Number(p.dataset.u) : null,
        after,
        container: cell?.matches('td') ? Number(cell.dataset.u) : cell?.dataset.root != null ? Number(cell.dataset.root) : null,
        style: p.dataset.style ?? '',
        num: p.dataset.num ?? '',
        align: p.dataset.align ?? '',
        ind: p.dataset.ind ?? '',
        line: p.dataset.line ?? '',
        items: itemsOf(p),
      });
    }
    return { present, paras: out, tables };
  }

  // The caret as (paragraph, character), to put it back after the page is rebuilt.
  function caretPos() {
    const sel = getSelection();
    if (!sel.rangeCount || !inside(sel.anchorNode)) return null;
    const p = (sel.anchorNode.nodeType === 1 ? sel.anchorNode : sel.anchorNode.parentElement).closest('p');
    if (!p) return null;
    const range = document.createRange();
    range.setStart(p, 0);
    range.setEnd(sel.anchorNode, sel.anchorOffset);
    return { index: allIn('p').indexOf(p), offset: range.toString().length };
  }
  function restoreCaret(pos) {
    const p = pos && allIn('p')[pos.index];
    if (!p) return;
    const walker = document.createTreeWalker(p, NodeFilter.SHOW_TEXT);
    let left = pos.offset;
    for (let n = walker.nextNode(); n; n = walker.nextNode()) {
      if (left <= n.data.length) {
        getSelection().collapse(n, left);
        return;
      }
      left -= n.data.length;
    }
    getSelection().collapse(p, 0);
  }

  function render(next, keep = false) {
    view = next;
    const caret = keep ? caretPos() : null;
    const scroll = stage.scrollTop;
    observer.disconnect();
    styleEl.textContent = view.css.replaceAll('.dx-page', `.dx-page[data-doc="${docKey}"]`);
    // Built by jay_docs::doc::edit, which escapes every text and attribute.
    unpick();
    body.innerHTML = view.html;
    const pg = view.page;
    for (const [el, margin] of [
      [header, view.header],
      [footer, view.footer],
    ]) {
      el.hidden = !margin;
      el.innerHTML = margin?.html ?? '';
      if (margin) el.dataset.root = String(margin.u);
      Object.assign(el.style, { left: `${pg.left}pt`, width: `${pg.w - pg.left - pg.right}pt` });
    }
    header.style.top = `${pg.header}pt`;
    footer.style.top = `${pg.h - pg.footer}pt`;
    renumber();
    snapshot();
    dirty.clear();
    changed = false;
    past.length = 0;
    future.length = 0;
    for (const el of editables()) observer.observe(el, WATCH);
    fillMenus();
    showDirty();
    stage.scrollTop = scroll;
    if (caret) restoreCaret(caret);
  }

  let saving = null;
  async function save(force = false) {
    if (saving) return saving;
    status.textContent = t('Saving…');
    saving = (async () => {
      try {
        render(await call('docx_edit_save', { id, edits: collect(), force }), true);
        status.textContent = t('Saved');
        return true;
      } catch (error) {
        if (error?.kind === 'changed') {
          saving = null;
          if (confirm(t('{name} was changed by another program. Save over it?', { name: file.name }))) return save(true);
          status.textContent = '';
          return false;
        }
        status.textContent = t("Couldn't save: {error}", { error: error?.message ?? error?.kind ?? String(error) });
        return false;
      } finally {
        saving = null;
      }
    })();
    return saving;
  }

  // ----- The toolbar -----
  let lastRange = null;
  const keepSelection = (el) => {
    // Clicking a button mustn't take the selection away from the page.
    el.addEventListener('mousedown', (e) => e.preventDefault());
    return el;
  };
  const button = (icon, label, onClick, extra = {}) => keepSelection(h('button', { type: 'button', class: 'icon-btn', title: label, 'aria-label': label, html: icons[icon], onClick, ...extra }));
  const sep = () => h('span', { class: 'dx-sep', 'aria-hidden': 'true' });

  function restoreRange() {
    if (!lastRange) return;
    const box = editables().find((e) => e.contains(lastRange.startContainer)) ?? body;
    box.focus({ preventScroll: true });
    const sel = getSelection();
    sel.removeAllRanges();
    sel.addRange(lastRange);
  }
  // ----- Undo and redo: the whole page, so paragraph settings, pictures and tables come back too -----
  const past = [];
  const future = [];
  let lastStep = { at: 0, kind: '' };
  const state = () => ({ html: editables().map((e) => e.innerHTML), caret: caretPos() });
  /** Remembers the page before a change; typing of one kind within a second is one step. */
  function checkpoint(kind = 'edit') {
    const now = Date.now();
    if (kind === 'typing' && lastStep.kind === 'typing' && now - lastStep.at < 1000) {
      lastStep.at = now;
      return;
    }
    lastStep = { at: now, kind };
    past.push(state());
    if (past.length > 200) past.shift();
    future.length = 0;
  }
  function travel(from, to) {
    const step = from.pop();
    if (!step) return;
    to.push(state());
    lastStep = { at: 0, kind: '' };
    unpick();
    // Rebuilt from what it was: the paragraphs that differ from before count as changed.
    note(observer.takeRecords());
    const before = new Map(allIn('p[data-u]').map((p) => [p.dataset.u, p.outerHTML]));
    editables().forEach((e, i) => (e.innerHTML = step.html[i] ?? e.innerHTML));
    observer.takeRecords();
    for (const p of allIn('p[data-u]')) if (before.get(p.dataset.u) !== p.outerHTML) dirty.add(p.dataset.u);
    if (!changed) {
      changed = true;
      showDirty();
    }
    renumber();
    restoreCaret(step.caret);
    refresh();
  }
  const undo = () => travel(past, future);
  const redo = () => travel(future, past);

  function exec(command, value = null) {
    checkpoint();
    restoreRange();
    document.execCommand('styleWithCSS', false, true);
    document.execCommand(command, false, value);
    refresh();
  }
  function selectedParas() {
    const sel = getSelection();
    const range = sel.rangeCount && inside(sel.anchorNode) ? sel.getRangeAt(0) : lastRange;
    if (!range) return [];
    return allIn('p').filter((p) => range.intersectsNode(p));
  }
  function eachPara(fn) {
    checkpoint();
    restoreRange();
    selectedParas().forEach(fn);
    renumber();
    refresh();
  }

  function setSize(pt) {
    pt = Math.min(Math.max(Math.round(pt * 2) / 2, 1), 400);
    checkpoint();
    exec('fontSize', '7');
    // execCommand only knows sizes 1 to 7: the 7 it made becomes the real size.
    for (const el of allIn('span[style*="xxx-large"], font[size="7"]')) {
      if (el.tagName === 'FONT') {
        const span = document.createElement('span');
        span.append(...el.childNodes);
        el.replaceWith(span);
        span.style.fontSize = `${pt}pt`;
      } else el.style.fontSize = `${pt}pt`;
    }
    refresh();
  }

  function setStyle(styleId) {
    const style = view.styles.find((s) => s.id === styleId);
    eachPara((p) => {
      p.dataset.style = styleId === view.default_style ? '' : styleId;
      p.className = `dx-p dx-p-${styleClass(styleId)}`;
      if (style?.num && !p.dataset.num) setList(p, `${style.num}:0`);
    });
  }

  function setList(p, num) {
    p.dataset.num = num;
    if (num) {
      const level = levelOf(num);
      if (!p.dataset.ind) p.style.marginLeft = `${level.left / 20}pt`;
      p.style.textIndent = `${-level.hanging / 20}pt`;
    } else {
      p.style.marginLeft = p.dataset.ind ? `${Number(p.dataset.ind) / 20}pt` : '';
      p.style.textIndent = '';
    }
  }

  function toggleList(bullet) {
    const isIt = (p) => p.dataset.num && (levelOf(p.dataset.num).fmt === 'bullet') === bullet;
    const paras = selectedParas();
    const off = paras.length && paras.every(isIt);
    const numId = bullet ? (view.bullet_num ?? 'new-bullet') : (view.number_num ?? 'new-number');
    eachPara((p) => setList(p, off ? '' : `${numId}:${p.dataset.num?.split(':')[1] ?? 0}`));
  }

  function indent(dir) {
    eachPara((p) => {
      if (p.dataset.num) {
        const [numId, lvl] = p.dataset.num.split(':');
        setList(p, `${numId}:${Math.min(Math.max((Number(lvl) || 0) + dir, 0), 8)}`);
        return;
      }
      const now = Math.round(parseFloat(getComputedStyle(p).marginLeft) * 15);
      const next = Math.max(0, Math.round((now + dir * 720) / 360) * 360);
      p.dataset.ind = String(next);
      p.style.marginLeft = `${next / 20}pt`;
    });
  }

  /** A picture from a file, at the cursor; drag it anywhere afterwards. */
  async function addImage() {
    restoreRange();
    const range = lastRange;
    let added;
    try {
      added = await call('docx_edit_add_image', { id });
    } catch (error) {
      status.textContent = t("Couldn't add the picture: {error}", { error: error?.message ?? error?.kind ?? String(error) });
      return;
    }
    if (added) placeImage(added, range);
  }

  /** Puts a picture Rust now keeps (`added`) at `range`, no wider than the space between the margins. */
  function placeImage(added, range) {
    checkpoint();
    const room = view.page.w - view.page.left - view.page.right;
    const scale = Math.min(1, room / added.w);
    const w = Math.round(added.w * scale * 100) / 100;
    const ht = Math.round(added.h * scale * 100) / 100;
    const atom = h('span', { class: 'dx-atom', contenteditable: 'false', 'data-new': String(added.n), 'data-img': '1', 'data-w': String(w), 'data-h': String(ht) }, h('img', { src: added.url, alt: '', style: `width:${w}pt;height:${ht}pt` }));
    const at = range && inside(range.startContainer) ? range : null;
    if (at) {
      at.collapse(false);
      at.insertNode(atom);
    } else {
      (body.querySelector('p:last-of-type') ?? body).append(atom);
    }
    pick(atom);
  }

  /** A table of `rows` × `cols` after the paragraph with the cursor, the cursor in its first cell. */
  function addTable(rows, cols) {
    checkpoint();
    restoreRange();
    const p = selectedParas()[0] ?? [...body.querySelectorAll('p')].at(-1);
    if (!p) return;
    const room = view.page.w - view.page.left - view.page.right;
    const width = Math.floor((room / cols) * 10) / 10;
    const normal = () => h('p', { class: `dx-p dx-p-${styleClass(view.default_style)}`, 'data-style': '', 'data-num': '', 'data-align': '', 'data-ind': '', 'data-line': '' }, h('br'));
    const table = h(
      'table',
      { class: `dx-table dx-t-${styleClass(view.table_style ?? '')} is-bordered` },
      h(
        'tbody',
        {},
        Array.from({ length: rows }, () => h('tr', {}, Array.from({ length: cols }, () => h('td', { style: `width:${width}pt` }, normal())))),
      ),
    );
    p.after(table);
    // Word needs a paragraph after a table, and so does the cursor.
    if (!table.nextElementSibling || table.nextElementSibling.tagName !== 'P') table.after(normal());
    getSelection().collapse(table.querySelector('p'), 0);
    refresh();
  }
  const gridCells = [];
  const gridLabel = h('div', { class: 'dx-grid-label' }, '1 × 1');
  const grid = h(
    'div',
    { class: 'dx-grid-picker', hidden: true, role: 'dialog', 'aria-label': t('Insert table') },
    h(
      'div',
      { class: 'dx-grid' },
      Array.from({ length: 8 }, (_, r) =>
        Array.from({ length: 10 }, (_, c) => {
          const cell = keepSelection(h('button', { type: 'button', class: 'dx-grid-cell', 'aria-label': `${r + 1} × ${c + 1}`, 'data-r': String(r + 1), 'data-c': String(c + 1) }));
          gridCells.push(cell);
          return cell;
        }),
      ).flat(),
    ),
    gridLabel,
  );
  grid.addEventListener('pointerover', (e) => {
    const cell = e.target.closest('.dx-grid-cell');
    if (!cell) return;
    const [r, c] = [Number(cell.dataset.r), Number(cell.dataset.c)];
    gridLabel.textContent = `${r} × ${c}`;
    for (const g of gridCells) g.classList.toggle('is-on', Number(g.dataset.r) <= r && Number(g.dataset.c) <= c);
  });
  grid.addEventListener('click', (e) => {
    const cell = e.target.closest('.dx-grid-cell');
    if (!cell) return;
    grid.hidden = true;
    addTable(Number(cell.dataset.r), Number(cell.dataset.c));
  });
  addEventListener('pointerdown', (e) => {
    if (!grid.hidden && !grid.contains(e.target) && !e.target.closest('.dx-table-button')) grid.hidden = true;
  });

  function addLink() {
    restoreRange();
    const href = prompt(t('Link address (https://…)'), 'https://');
    if (!href || !/^(https?:|mailto:|#)/i.test(href.trim())) return;
    exec('createLink', href.trim());
  }

  const undoButton = button('undo', t('Undo (Ctrl+Z)'), () => undo());
  const redoButton = button('redo', t('Redo (Ctrl+Y)'), () => redo());
  const printButton = button('print', t('Print (Ctrl+P)'), () => printIt());
  const zoomSelect = h(
    'select',
    { class: 'input dx-zoom', 'aria-label': t('Zoom'), title: t('Zoom') },
    h('option', { value: 'width' }, t('Fit width')),
    ZOOMS.map((z) => h('option', { value: String(z) }, `${Math.round(z * 100)}%`)),
  );
  const styleSelect = h('select', { class: 'input dx-style', 'aria-label': t('Paragraph style'), title: t('Paragraph style') });
  const fontSelect = h('select', { class: 'input dx-font', 'aria-label': t('Font'), title: t('Font') });
  const sizeInput = h('input', { class: 'input dx-size', type: 'number', min: '1', max: '400', step: '0.5', 'aria-label': t('Font size'), title: t('Font size') });
  const toggles = [
    ['bold', button('bold', t('Bold (Ctrl+B)'), () => exec('bold'))],
    ['italic', button('italic', t('Italic (Ctrl+I)'), () => exec('italic'))],
    ['underline', button('underline', t('Underline (Ctrl+U)'), () => exec('underline'))],
    ['strikeThrough', button('strike', t('Strikethrough'), () => exec('strikeThrough'))],
  ];
  const colorInput = h('input', { type: 'color', value: '#c00000', class: 'dx-color-input', tabindex: '-1', 'aria-hidden': 'true' });
  const highlightInput = h('input', { type: 'color', value: '#ffff00', class: 'dx-color-input', tabindex: '-1', 'aria-hidden': 'true' });
  const colorBar = h('span', { class: 'dx-color-bar' });
  const highlightBar = h('span', { class: 'dx-color-bar', style: 'background:#ffff00' });
  const colorButton = button('textColor', t('Text color'), () => colorInput.click());
  colorButton.append(colorBar);
  const highlightButton = button('highlight', t('Highlight color'), () => highlightInput.click());
  highlightButton.append(highlightBar);
  colorInput.addEventListener('input', () => exec('foreColor', colorInput.value));
  highlightInput.addEventListener('input', () => {
    highlightBar.style.background = highlightInput.value;
    exec('hiliteColor', highlightInput.value);
  });
  const alignButtons = ALIGNS.map(([value, icon, label]) =>
    button(icon, t(label), () =>
      eachPara((p) => {
        p.dataset.align = value;
        p.style.textAlign = value === 'both' ? 'justify' : value;
      }),
    ),
  );
  const spacingSelect = h(
    'select',
    { class: 'input dx-spacing', 'aria-label': t('Line spacing'), title: t('Line spacing') },
    h('option', { value: '', disabled: true, selected: true }, '↕'),
    SPACINGS.map((value) => h('option', { value: String(value) }, (value / 240).toLocaleString(document.documentElement.lang || undefined))),
  );
  spacingSelect.addEventListener('change', () => {
    const v = Number(spacingSelect.value);
    spacingSelect.value = '';
    eachPara((p) => {
      p.dataset.line = String(v);
      p.style.lineHeight = String(Math.round((v / 240) * 1.15 * 1000) / 1000);
    });
  });

  const status = h('span', { class: 'hint doc-status', role: 'status' });
  const dirtyMark = h('span', { class: 'editor-dirty', hidden: true, title: t('Not saved'), 'aria-label': t('Not saved') }, '●');
  const saveButton = h('button', { type: 'button', class: 'btn', onClick: () => save() }, t('Save'));
  const exportSelect = h(
    'select',
    { class: 'input', 'aria-label': t('Export as') },
    [
      ['pdf', 'PDF'],
      ['docx', 'Word (DOCX)'],
      ['markdown', 'Markdown'],
      ['html', 'HTML'],
      ['txt', t('Plain text')],
    ].map(([value, label]) => h('option', { value }, label)),
  );
  async function exportIt() {
    if (exportSelect.value === 'pdf') return printIt();
    if (changed && !(await save())) return;
    try {
      const where = await call('docx_edit_export', { id, format: exportSelect.value });
      if (where) status.textContent = t('Saved as {path}', { path: where });
    } catch (error) {
      status.textContent = t("Couldn't export: {error}", { error: error?.message ?? error?.kind ?? String(error) });
    }
  }
  function showDirty() {
    dirtyMark.hidden = !changed;
    saveButton.disabled = !changed;
  }

  function fillMenus() {
    const choices = view.styles.filter((s) => s.quick);
    styleSelect.replaceChildren(...choices.map((s) => h('option', { value: s.id }, styleName(s.name))));
    const fonts = [...new Set([...FONTS, ...view.fonts])].sort((a, b) => a.localeCompare(b));
    fontSelect.replaceChildren(...fonts.map((f) => h('option', { value: f, style: `font-family:"${f.replace(/["\\]/g, '')}"` }, f)));
  }
  styleSelect.addEventListener('change', () => setStyle(styleSelect.value));
  fontSelect.addEventListener('change', () => exec('fontName', fontSelect.value));
  sizeInput.addEventListener('change', () => setSize(Number(sizeInput.value) || 11));
  const sizeDown = button('minus', t('Smaller'), () => setSize((Number(sizeInput.value) || 11) - 1));
  const sizeUp = button('plus', t('Bigger'), () => setSize((Number(sizeInput.value) || 11) + 1));

  const toolbar = h(
    'div',
    { class: 'doc-toolbar dx-toolbar', role: 'toolbar', 'aria-label': t('Document tools') },
    h('span', { class: 'img-tool-group' }, undoButton, redoButton, printButton),
    sep(),
    zoomSelect,
    sep(),
    styleSelect,
    sep(),
    fontSelect,
    sep(),
    h('span', { class: 'img-tool-group dx-size-group' }, sizeDown, sizeInput, sizeUp),
    sep(),
    h('span', { class: 'img-tool-group' }, toggles.map(([, b]) => b), colorButton, highlightButton, colorInput, highlightInput),
    sep(),
    button('link', t('Insert link (Ctrl+K)'), addLink),
    button('image', t('Insert image'), addImage),
    h('span', { class: 'dx-table-menu' }, button('table', t('Insert table'), () => (grid.hidden = !grid.hidden), { class: 'icon-btn dx-table-button' }), grid),
    sep(),
    h('span', { class: 'img-tool-group' }, alignButtons, spacingSelect),
    sep(),
    h(
      'span',
      { class: 'img-tool-group' },
      button('listBullet', t('Bulleted list'), () => toggleList(true)),
      button('listNumber', t('Numbered list'), () => toggleList(false)),
      button('outdent', t('Decrease indent'), () => indent(-1)),
      button('indent', t('Increase indent'), () => indent(1)),
      button('clearFormat', t('Clear formatting'), () => exec('removeFormat')),
    ),
    h('span', { class: 'dx-toolbar-end' }, dirtyMark, status, exportSelect, h('button', { type: 'button', class: 'btn btn-secondary', onClick: exportIt }, t('Export')), saveButton),
  );

  /** The toolbar shows what is under the cursor. */
  function refresh() {
    const sel = getSelection();
    if (!sel.rangeCount || !inside(sel.anchorNode)) return;
    lastRange = sel.getRangeAt(0).cloneRange();
    const el = sel.anchorNode.nodeType === 1 ? sel.anchorNode : sel.anchorNode.parentElement;
    const p = el.closest('p');
    const cs = getComputedStyle(el);
    for (const [command, b] of toggles) b.setAttribute('aria-pressed', String(document.queryCommandState(command)));
    const font = firstFamily(cs.fontFamily);
    if (![...fontSelect.options].some((o) => o.value === font)) fontSelect.append(h('option', { value: font }, font));
    fontSelect.value = font;
    sizeInput.value = String(Math.round(parseFloat(cs.fontSize) * 0.75 * 2) / 2);
    colorBar.style.background = cs.color;
    if (p) {
      const style = p.dataset.style || view.default_style;
      if (![...styleSelect.options].some((o) => o.value === style)) styleSelect.append(h('option', { value: style }, styleName(view.styles.find((s) => s.id === style)?.name || style)));
      styleSelect.value = style;
      const align = getComputedStyle(p).textAlign;
      const now = { start: 'left', left: 'left', center: 'center', right: 'right', end: 'right', justify: 'both' }[align] ?? 'left';
      ALIGNS.forEach(([value], i) => alignButtons[i].setAttribute('aria-pressed', String(value === now)));
    }
  }
  let refreshQueued = false;
  const onSelection = () => {
    if (refreshQueued) return;
    refreshQueued = true;
    requestAnimationFrame(() => {
      refreshQueued = false;
      refresh();
    });
  };
  document.addEventListener('selectionchange', onSelection);

  // ----- Zoom -----
  let zoom = 'width';
  function layout() {
    page.style.zoom = '1';
    const natural = page.offsetWidth;
    const fit = (stage.clientWidth - 48) / natural;
    const z = zoom === 'width' ? Math.max(Math.min(fit, 1.5), 0.3) : zoom;
    page.style.zoom = String(z);
    if (picked) showFrame();
  }
  zoomSelect.addEventListener('change', () => {
    zoom = zoomSelect.value === 'width' ? 'width' : Number(zoomSelect.value);
    layout();
  });

  // ----- Print / PDF -----
  function printIt() {
    const printRoot = h('div', { id: 'print-root', class: 'dx-print' }, h('style', {}, styleEl.textContent), page.cloneNode(true));
    printRoot.querySelector('.dx-page').style.zoom = '1';
    document.body.append(printRoot);
    const title = document.title;
    document.title = file.name.replace(/\.[^.]+$/, '');
    addEventListener(
      'afterprint',
      () => {
        printRoot.remove();
        document.title = title;
      },
      { once: true },
    );
    window.print();
  }

  // ----- Pictures: click to pick, drag anywhere on the page, corners to resize -----
  // A picture dragged out of the text floats where it's dropped (anchored to the paragraph there,
  // over the text); Shift while resizing lets go of the proportions.
  const PT = 0.75; // points per CSS pixel
  const frame = h(
    'div',
    { class: 'dx-frame', hidden: true },
    ['nw', 'ne', 'sw', 'se'].map((corner) => h('span', { class: `dx-handle is-${corner}`, 'data-corner': corner })),
  );
  page.append(frame);
  let picked = null;
  /** The picture under the mouse: not put back in place until it's dropped. */
  let dragging = null;
  const zoomNow = () => parseFloat(page.style.zoom) || 1;
  /** Where an element is on the page, in the page's own CSS pixels (zoom undone). */
  function rectIn(el) {
    const pr = page.getBoundingClientRect();
    const r = el.getBoundingClientRect();
    const z = zoomNow();
    return { x: (r.left - pr.left) / z, y: (r.top - pr.top) / z, w: r.width / z, h: r.height / z };
  }
  const picOf = (atom) => atom.querySelector('img, .dx-shape') ?? atom;

  /** Puts a floating picture where the file says (points from the page, its margins or its paragraph). */
  function placeFloat(atom) {
    const pg = view.page;
    const d = atom.dataset;
    const w = Number(d.w) || 0;
    const ht = Number(d.h) || 0;
    const p = atom.closest('p');
    const paraTop = p ? rectIn(p).y * PT : 0;
    // The page the paragraph is on.
    const top0 = Math.max(0, Math.floor(paraTop / (pg.h + GAP))) * (pg.h + GAP);
    const across = { page: [0, pg.w], leftMargin: [0, pg.left], rightMargin: [pg.w - pg.right, pg.right] }[d.hr] ?? [pg.left, pg.w - pg.left - pg.right];
    const down =
      { page: [top0, pg.h], margin: [top0 + pg.top, pg.h - pg.top - pg.bottom], topMargin: [top0, pg.top], bottomMargin: [top0 + pg.h - pg.bottom, pg.bottom] }[d.vr] ?? [paraTop, 0];
    const x = d.ha ? across[0] + ({ center: (across[1] - w) / 2, right: across[1] - w, outside: across[1] - w }[d.ha] ?? 0) : across[0] + (Number(d.hx) || 0);
    const y = d.va ? down[0] + ({ center: (down[1] - ht) / 2, bottom: down[1] - ht }[d.va] ?? 0) : down[0] + (Number(d.vy) || 0);
    atom.style.left = `${x}pt`;
    atom.style.top = `${y}pt`;
  }
  function layoutFloats() {
    for (const atom of allIn('.dx-atom[data-float]')) if (atom !== dragging) placeFloat(atom);
    if (picked) showFrame();
  }
  function showFrame() {
    if (!picked?.isConnected) return unpick();
    const r = rectIn(picOf(picked));
    Object.assign(frame.style, { left: `${r.x}px`, top: `${r.y}px`, width: `${r.w}px`, height: `${r.h}px` });
    frame.hidden = false;
  }
  function pick(atom) {
    picked = atom;
    showFrame();
  }
  function unpick() {
    picked = null;
    frame.hidden = true;
  }
  function sizePicture(atom, wPx, hPx) {
    const pic = picOf(atom);
    pic.style.width = `${wPx * PT}pt`;
    pic.style.height = `${hPx * PT}pt`;
    atom.dataset.w = String(Math.round(wPx * PT * 100) / 100);
    atom.dataset.h = String(Math.round(hPx * PT * 100) / 100);
  }
  /** The paragraph at a height on the page (the innermost one, e.g. in a cell), or the closest. */
  function paragraphAt(yPx) {
    let best = null;
    let gap = Infinity;
    for (const p of body.querySelectorAll('p')) {
      const r = rectIn(p);
      const d = yPx < r.y ? r.y - yPx : yPx > r.y + r.h ? yPx - r.y - r.h : 0;
      if (d <= gap) {
        gap = d;
        best = p;
      }
    }
    return best;
  }
  /** After a move or resize: the picture's place in file terms, anchored to the paragraph it's on. */
  function settle(atom) {
    atom.dataset.geo = '1';
    if (atom.dataset.float) {
      const r = rectIn(picOf(atom));
      const from = atom.closest('p');
      const to = paragraphAt(r.y) ?? from;
      if (to && to !== from) {
        to.prepend(atom);
        if (from && !from.textContent && !from.querySelector('.dx-atom:not([hidden]), img')) from.replaceChildren(document.createElement('br'));
      }
      const paraTop = to ? rectIn(to).y : 0;
      for (const k of ['ha', 'va']) delete atom.dataset[k];
      Object.assign(atom.dataset, { moved: '1', hr: 'page', hx: String(Math.round(r.x * PT * 10) / 10), vr: 'paragraph', vy: String(Math.round((r.y - paraTop) * PT * 10) / 10) });
      placeFloat(atom);
    }
    showFrame();
  }
  page.addEventListener('pointerdown', (e) => {
    const handle = e.target.closest('.dx-handle');
    const atom = handle ? picked : e.target.closest('.dx-atom[data-img]');
    if (!atom || !inside(atom)) {
      if (picked) unpick();
      return;
    }
    e.preventDefault();
    pick(atom);
    const z = zoomNow();
    const start = { x: e.clientX, y: e.clientY, r: rectIn(picOf(atom)) };
    const corner = handle?.dataset.corner;
    const ratio = start.r.h / start.r.w || 1;
    let changed = false;
    const onMove = (ev) => {
      const dx = (ev.clientX - start.x) / z;
      const dy = (ev.clientY - start.y) / z;
      if (!changed && Math.hypot(dx, dy) < 3) return;
      if (!changed) checkpoint();
      changed = true;
      dragging = atom;
      if (corner) {
        const sx = corner.includes('w') ? -1 : 1;
        const sy = corner.includes('n') ? -1 : 1;
        const w = Math.max(12, start.r.w + sx * dx);
        const ht = ev.shiftKey ? Math.max(12, start.r.h + sy * dy) : w * ratio;
        sizePicture(atom, w, ht);
        if (atom.dataset.float) {
          atom.style.left = `${sx < 0 ? start.r.x + start.r.w - w : start.r.x}px`;
          atom.style.top = `${sy < 0 ? start.r.y + start.r.h - ht : start.r.y}px`;
        }
      } else {
        // Out of the line it was in, free on the page.
        atom.dataset.float = '1';
        atom.style.left = `${start.r.x + dx}px`;
        atom.style.top = `${start.r.y + dy}px`;
      }
      showFrame();
    };
    const onUp = () => {
      removeEventListener('pointermove', onMove);
      removeEventListener('pointerup', onUp);
      dragging = null;
      if (changed) settle(atom);
    };
    addEventListener('pointermove', onMove);
    addEventListener('pointerup', onUp);
  });

  /** Pasted HTML as this page's paragraphs and spans, with only the formatting the toolbar knows. */
  function cleanPaste(html) {
    const doc = new DOMParser().parseFromString(html, 'text/html');
    const BLOCKS = new Set(['P', 'DIV', 'H1', 'H2', 'H3', 'H4', 'H5', 'H6', 'LI', 'BLOCKQUOTE', 'PRE', 'TR', 'DT', 'DD']);
    const out = [];
    let line = null;
    const attr = (v) => String(v).replace(/[&"<>]/g, (c) => ({ '&': '&amp;', '"': '&quot;', '<': '&lt;', '>': '&gt;' })[c]);
    const text = (v) => String(v).replace(/[&<>]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;' })[c]);
    const open = (align) => {
      line = { align, html: '' };
      out.push(line);
    };
    const styleOf = (el, inherited) => {
      const css = { ...inherited };
      const st = el.style;
      const tag = el.tagName;
      if (tag === 'B' || tag === 'STRONG' || /^(bold|[6-9]00)$/.test(st.fontWeight)) css['font-weight'] = '700';
      if (st.fontWeight === 'normal' || st.fontWeight === '400') css['font-weight'] = '400';
      if (tag === 'I' || tag === 'EM' || st.fontStyle === 'italic') css['font-style'] = 'italic';
      const deco = new Set((css['text-decoration-line'] ?? '').split(' ').filter(Boolean));
      if (tag === 'U' || /underline/.test(st.textDecoration)) deco.add('underline');
      if (tag === 'S' || tag === 'STRIKE' || tag === 'DEL' || /line-through/.test(st.textDecoration)) deco.add('line-through');
      if (deco.size) css['text-decoration-line'] = [...deco].join(' ');
      if (tag === 'SUP' || st.verticalAlign === 'super') css['vertical-align'] = 'super';
      if (tag === 'SUB' || st.verticalAlign === 'sub') css['vertical-align'] = 'sub';
      if (/^#[0-9a-f]{3,6}$|^rgb\(/i.test(st.color)) css.color = st.color;
      const bg = st.backgroundColor || st.background;
      if (/^#[0-9a-f]{3,6}$|^rgb\(/i.test(bg) && toHex(bg) !== 'ffffff') css['background-color'] = bg;
      const font = firstFamily(st.fontFamily).replace(/[^\w .-]/g, '');
      if (font) css['font-family'] = `'${font}'`;
      const size = /^([\d.]+)(pt|px)$/.exec(st.fontSize);
      if (size) css['font-size'] = `${size[2] === 'px' ? Number(size[1]) * 0.75 : Number(size[1])}pt`;
      return css;
    };
    const walk = (node, css, href) => {
      for (const n of node.childNodes) {
        if (n.nodeType === 3) {
          const value = n.data.replace(/\s+/g, ' ');
          if (!value.trim() && !line?.html) continue;
          if (!line) open('');
          const style = Object.entries(css).map(([k, v]) => `${k}:${v}`).join(';');
          const span = style ? `<span style="${attr(style)}">${text(value)}</span>` : text(value);
          line.html += href ? `<a href="${attr(href)}">${span}</a>` : span;
        } else if (n.nodeType === 1) {
          const tag = n.tagName;
          if (['SCRIPT', 'STYLE', 'TITLE', 'META', 'IMG', 'SVG', 'IFRAME', 'OBJECT'].includes(tag)) continue;
          if (tag === 'BR') {
            if (line) line.html += '<br>';
            continue;
          }
          const link = tag === 'A' && /^(https?:|mailto:)/i.test(n.getAttribute('href') ?? '') ? n.getAttribute('href') : href;
          if (BLOCKS.has(tag)) {
            const align = { center: 'center', right: 'right', justify: 'both' }[n.style.textAlign] ?? '';
            open(align);
            walk(n, styleOf(n, css), link);
            line = null;
          } else if (tag === 'TD' || tag === 'TH') {
            if (line?.html) line.html += '\t';
            walk(n, styleOf(n, css), link);
          } else {
            walk(n, styleOf(n, css), link);
          }
        }
      }
    };
    walk(doc.body, {}, null);
    const lines = out.filter((l) => l.html.replace(/<[^>]*>/g, '').trim());
    if (!lines.length) return '';
    const cls = `dx-p dx-p-${styleClass(view.default_style)}`;
    // One line pastes into the paragraph; more become paragraphs of normal text.
    if (lines.length === 1) return lines[0].html;
    return lines
      .map((l) => `<p class="${cls}" data-style="" data-num="" data-align="${l.align}" data-ind="" data-line=""${l.align ? ` style="text-align:${l.align === 'both' ? 'justify' : l.align}"` : ''}>${l.html}</p>`)
      .join('');
  }

  // ----- Typing -----
  page.addEventListener('paste', async (e) => {
    e.preventDefault();
    // A picture (a screenshot, say) goes in as a picture.
    const file = [...(e.clipboardData?.files ?? [])].find((f) => f.type.startsWith('image/'));
    if (file) {
      const range = getSelection().rangeCount ? getSelection().getRangeAt(0).cloneRange() : lastRange;
      try {
        placeImage(await call('docx_edit_paste_image', { id, bytes: [...new Uint8Array(await file.arrayBuffer())] }), range);
      } catch (error) {
        status.textContent = t("Couldn't add the picture: {error}", { error: error?.message ?? error?.kind ?? String(error) });
      }
      return;
    }
    // Formatted text (from Word, a web page, another document) keeps its look; the markup is rebuilt
    // from scratch, so nothing of the source's (scripts, classes, data-u) comes along.
    const html = e.clipboardData.getData('text/html');
    const clean = html ? cleanPaste(html) : '';
    checkpoint();
    if (clean) document.execCommand('insertHTML', false, clean);
    else document.execCommand('insertText', false, e.clipboardData.getData('text/plain'));
  });
  page.addEventListener('drop', (e) => e.preventDefault());
  page.addEventListener('beforeinput', (e) => {
    if (e.inputType === 'historyUndo' || e.inputType === 'historyRedo') {
      e.preventDefault();
      return e.inputType === 'historyUndo' ? undo() : redo();
    }
    checkpoint(e.inputType === 'insertText' || e.inputType.startsWith('delete') ? 'typing' : 'edit');
  });
  // Enter at the end of a heading starts the style's following one (normal text), as in Word.
  page.addEventListener('input', (e) => {
    if (e.inputType !== 'insertParagraph') return;
    const sel = getSelection();
    const p = sel.anchorNode && (sel.anchorNode.nodeType === 1 ? sel.anchorNode : sel.anchorNode.parentElement).closest('p');
    const prev = p?.previousElementSibling;
    if (!p || prev?.tagName !== 'P' || p.textContent || p.dataset.style !== prev.dataset.style) return;
    const next = styleOf(prev)?.next;
    if (!next || next === (prev.dataset.style || view.default_style)) return;
    p.dataset.style = next === view.default_style ? '' : next;
    p.className = `dx-p dx-p-${styleClass(next)}`;
    // Inline formatting typed in the heading doesn't come along either.
    if (!p.querySelector('img, .dx-atom')) p.replaceChildren(document.createElement('br'));
    refresh();
  });
  page.addEventListener('click', (e) => {
    const a = e.target.closest('a[href]');
    if (a && (e.ctrlKey || e.metaKey) && /^(https?:|mailto:)/i.test(a.getAttribute('href'))) {
      e.preventDefault();
      openLink(a.getAttribute('href')).catch(() => {});
    }
  });
  const root = h('div', { class: 'doc-viewer dx-editor' }, styleEl, toolbar, stage);
  root.addEventListener('keydown', (e) => {
    if (picked && (e.key === 'Delete' || e.key === 'Backspace')) {
      e.preventDefault();
      const atom = picked;
      unpick();
      checkpoint();
      atom.remove();
      return;
    }
    if (picked && e.key === 'Escape') return unpick();
    if (picked && !e.ctrlKey && !e.metaKey && e.key.length === 1) unpick();
    if (e.key === 'Tab' && inside(e.target)) {
      e.preventDefault();
      const p = selectedParas()[0];
      if (p?.dataset.num) indent(e.shiftKey ? -1 : 1);
      else if (!e.shiftKey) {
        checkpoint('typing');
        document.execCommand('insertText', false, '\t');
      }
      return;
    }
    if (!(e.ctrlKey || e.metaKey) || e.altKey) return;
    const key = e.key.toLowerCase();
    if (key === 'z' && !e.shiftKey) {
      e.preventDefault();
      undo();
    } else if (key === 'y' || (key === 'z' && e.shiftKey)) {
      e.preventDefault();
      redo();
    } else if (key === 's') {
      e.preventDefault();
      save();
    } else if (key === 'p') {
      e.preventDefault();
      printIt();
    } else if (key === 'k') {
      e.preventDefault();
      addLink();
    }
  });

  document.execCommand('defaultParagraphSeparator', false, 'p');
  render(view);
  new ResizeObserver(() => {
    if (!stage.isConnected || !stage.clientWidth) return;
    layout();
    scheduleRenumber();
  }).observe(stage);
  root.isDirty = () => changed;
  root.dispose = () => {
    observer.disconnect();
    document.removeEventListener('selectionchange', onSelection);
    call('doc_close', { id }).catch(() => {});
  };
  return root;
}
