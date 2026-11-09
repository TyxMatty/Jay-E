// Editing in the PDF viewer (stage 7): the pages (an organizer to reorder, turn, delete, insert and
// take pages out), annotations (highlight, draw, note, text, erase) and form fields. Stage 10: the
// page's own text and pictures (jay_docs::pdf_content), and filling in the blanks of a printed form;
// redaction (what's under a box leaves the file), a drawn signature, compressing, and OCR.
//
// Every change goes to Rust (src-tauri/src/pdf_edit.rs → jay_docs::pdf_edit), which makes the new
// PDF and opens it as a new document; the viewer is rebuilt around it (`rebuild`) where it was. So
// what's shown is always the real file, annotations drawn by the same renderer as the rest.
import { h } from './dom.js';
import { icons } from './icons.js';
import { t } from '../core/i18n.js';
import { fetchPage } from '../core/pdf.js';
import { settings } from '../store/settings.js';
import { formatSize } from '../core/files.js';

const call = async (cmd, args) => (await import('@tauri-apps/api/core')).invoke(cmd, args);
const COLORS = { highlight: '#ffd400', draw: '#e53935', note: '#ffd54f', text: '#1a73e8', fill: '#1a1a1a', sign: '#1a237e' };
/** How wide a signature is put on the page, in points. */
const SIGNATURE_WIDTH = 150;
/** Tools that work on what the page itself draws (its lines, pictures and blanks). */
const CONTENT_TOOLS = new Set(['editText', 'pictures', 'fill']);
/** A CSS family like the PDF font's, for the box the text is edited in. */
const familyOf = (font) => (/courier|mono|consola/i.test(font) ? 'monospace' : /times|serif|georgia|garamond|cambria|roman|book|minion|palatino/i.test(font) && !/sans/i.test(font) ? 'serif' : 'sans-serif');
const round = (v) => Math.round(v * 100) / 100;
const ORGANIZER_WIDTH = 150;

/** The error a change came back with, in words. */
function problem(error) {
  if (error?.kind === 'encrypted') return t("Password-protected PDFs can't be edited.");
  if (error?.kind === 'empty') return t('A PDF needs at least one page.');
  if (error?.message === 'already small') return t('The PDF is already as small as Jay-E can make it.');
  if (error?.message === 'already searchable') return t('Every page already has text.');
  if (/no OCR language/.test(error?.message ?? '')) return t('Windows has no text recognition for your languages. Add one in Settings › Time & language › Language.');
  if (/can't be redacted|can't redact/.test(error?.message ?? '')) return t("This part of the page can't be redacted safely, so nothing was changed.");
  return t("Couldn't change it: {error}", { error: error?.message ?? error?.kind ?? String(error) });
}

/**
 * The editing tools for one viewer build.
 * @param {object} o
 * @param {number} o.doc the shown document
 * @param {[number, number][]} o.sizes page sizes in points
 * @param {{ el: HTMLElement, index: number }[]} o.pages the page elements
 * @param {() => number} o.scale CSS px per point
 * @param {HTMLElement} o.scroller
 * @param {object} o.editor state kept across rebuilds: tool, colors, dirty, canUndo, organizer, selected
 * @param {(edited: { doc: number, pages: [number, number][] }) => void} o.rebuild
 */
export function createPdfTools({ doc, sizes, pages, scale, scroller, editor, rebuild }) {
  editor.tool ??= 'select';
  editor.colors ??= { ...COLORS };
  editor.textSize ??= 14;
  editor.fillSize ??= 12;
  editor.mark ??= 'check';
  editor.selected ??= new Set();
  let busy = false;

  const status = h('span', { class: 'hint doc-status', role: 'status' });
  const dirtyMark = h('span', { class: 'editor-dirty', hidden: !editor.dirty, title: t('Not saved'), 'aria-label': t('Not saved') }, '●');

  /** Sends a change; on success the viewer is rebuilt on the new document. */
  async function change(cmd, args, message = '') {
    if (busy) return false;
    busy = true;
    status.textContent = t('Applying…');
    try {
      const edited = await call(cmd, { doc, ...args });
      if (!edited) {
        status.textContent = '';
        return false;
      }
      editor.dirty = edited.dirty;
      editor.canUndo = edited.can_undo;
      editor.message = message;
      rebuild(edited);
      return true;
    } catch (error) {
      status.textContent = problem(error);
      return false;
    } finally {
      busy = false;
    }
  }
  const apply = (op) => change('pdf_edit', { op });

  async function save(force = false) {
    if (busy) return;
    busy = true;
    status.textContent = t('Saving…');
    try {
      const saved = await call('pdf_edit_save', { doc, force });
      editor.dirty = saved.dirty;
      dirtyMark.hidden = !editor.dirty;
      saveButton.disabled = !editor.dirty;
      status.textContent = t('Saved');
    } catch (error) {
      busy = false;
      if (error?.message === 'changed') {
        if (confirm(t('This file was changed by another program. Save over it?'))) return save(true);
        status.textContent = '';
        return;
      }
      status.textContent = problem(error);
    } finally {
      busy = false;
    }
  }

  // ----- The edit bar -----
  const toolButton = (tool, icon, label) =>
    h('button', {
      type: 'button',
      class: 'icon-btn',
      title: label,
      'aria-label': label,
      'aria-pressed': String(editor.tool === tool),
      html: icons[icon],
      'data-tool': tool,
      onClick: () => setTool(tool),
    });
  const tools = [
    toolButton('select', 'pointer', t('Select text')),
    toolButton('highlight', 'highlight', t('Highlight (select text)')),
    toolButton('draw', 'pen', t('Draw')),
    toolButton('note', 'note', t('Note')),
    toolButton('text', 'textTool', t('Text')),
    toolButton('erase', 'eraser', t('Erase an annotation')),
    toolButton('editText', 'editText', t("Edit the page's text")),
    toolButton('pictures', 'image', t('Move, replace or delete pictures')),
    toolButton('fill', 'fillIn', t('Fill in')),
    toolButton('redact', 'redact', t('Redact')),
    toolButton('sign', 'signature', t('Sign')),
  ];
  const colorInput = h('input', { type: 'color', class: 'pdf-color', title: t('Color'), 'aria-label': t('Color') });
  colorInput.addEventListener('input', () => (editor.colors[editor.tool] = colorInput.value));
  const sizeSelect = h(
    'select',
    { class: 'input', title: t('Text size'), 'aria-label': t('Text size'), onChange: (e) => (editor[editor.tool === 'fill' ? 'fillSize' : 'textSize'] = Number(e.target.value)) },
    [8, 10, 12, 14, 18, 24, 32].map((s) => h('option', { value: String(s) }, `${s} pt`)),
  );
  // Fill in: the mark a box gets, and today's date for the blank being typed in.
  const markSelect = h(
    'select',
    { class: 'input', title: t('Mark for boxes'), 'aria-label': t('Mark for boxes'), onChange: (e) => (editor.mark = e.target.value) },
    h('option', { value: 'check', selected: editor.mark === 'check' }, '✓'),
    h('option', { value: 'cross', selected: editor.mark === 'cross' }, '✗'),
  );
  const dateButton = h('button', { type: 'button', class: 'btn btn-secondary', title: t("Write today's date"), onPointerdown: (e) => e.preventDefault(), onClick: () => insertDate() }, t('Date'));
  // Pictures: what to do with the chosen one.
  const replaceButton = h('button', { type: 'button', class: 'btn btn-secondary', onClick: () => replacePicture() }, t('Replace…'));
  const deletePictureButton = h('button', { type: 'button', class: 'btn btn-secondary', onClick: () => deletePicture() }, t('Delete'));
  const fillGroup = h('span', { class: 'tb-group', role: 'group', 'aria-label': t('Fill in') }, markSelect, dateButton);
  const signGroup = h('span', { class: 'tb-group', role: 'group', 'aria-label': t('Sign') }, h('button', { type: 'button', class: 'btn btn-secondary', onClick: () => drawSignature() }, t('New signature…')));
  // The whole file: smaller, or searchable.
  const compressButton = h('button', { type: 'button', class: 'btn btn-secondary', title: t('Pictures at 150 dpi, everything compressed'), onClick: () => compress() }, t('Compress'));
  const ocrButton = h('button', { type: 'button', class: 'btn btn-secondary', title: t('Reads the pages that are only pictures (scans) and puts their text in, so it can be searched and copied'), onClick: () => makeSearchable() }, t('Make searchable'));
  const pictureGroup = h('span', { class: 'tb-group', role: 'group', 'aria-label': t('Picture') }, replaceButton, deletePictureButton);
  function setTool(tool) {
    editor.tool = tool;
    for (const b of tools) b.setAttribute('aria-pressed', String(b.dataset.tool === tool));
    scroller.dataset.tool = tool;
    colorInput.hidden = !COLORS[tool];
    if (COLORS[tool]) colorInput.value = editor.colors[tool];
    sizeSelect.hidden = tool !== 'text' && tool !== 'fill';
    sizeSelect.value = String(tool === 'fill' ? editor.fillSize : editor.textSize);
    fillGroup.hidden = tool !== 'fill';
    signGroup.hidden = tool !== 'sign';
    pictureGroup.hidden = tool !== 'pictures';
    replaceButton.disabled = deletePictureButton.disabled = !editor.picture;
    const hints = {
      editText: t('Click a line of text to change it.'),
      pictures: t('Drag a picture to move it; its corner sizes it.'),
      fill: t('Click a blank or anywhere to write; click a box to mark it.'),
      redact: t('Drag a box over what to remove: the text and pictures under it leave the file, not just covered.'),
      sign: t('Click where the signature goes.'),
    };
    status.textContent = editor.message || hints[tool] || '';
    layered.forEach(decorate);
    if (tool === 'sign' && !settings.get().signature) drawSignature();
  }
  const pagesButton = h('button', { type: 'button', class: 'btn btn-secondary', 'aria-pressed': String(!!editor.organizer), onClick: () => toggleOrganizer() }, h('span', { class: 'btn-icon', html: icons.pages }), t('Pages'));
  const undoButton = h('button', { type: 'button', class: 'icon-btn', title: t('Undo (Ctrl+Z)'), 'aria-label': t('Undo (Ctrl+Z)'), html: icons.undo, disabled: !editor.canUndo, onClick: () => undo() });
  const saveButton = h('button', { type: 'button', class: 'btn', disabled: !editor.dirty, onClick: () => save() }, t('Save'));
  const undo = () => editor.canUndo && change('pdf_edit_undo', {});
  const bar = h(
    'div',
    { class: 'doc-toolbar pdf-edit-bar', role: 'toolbar', 'aria-label': t('PDF editing') },
    pagesButton,
    h('span', { class: 'tb-group', role: 'group', 'aria-label': t('Tools') }, tools, colorInput, sizeSelect),
    fillGroup,
    pictureGroup,
    signGroup,
    h('span', { class: 'tb-group', role: 'group', 'aria-label': t('History') }, undoButton),
    h('span', { class: 'tb-group', role: 'group', 'aria-label': t('The whole file') }, compressButton, ocrButton),
    h('span', { class: 'dx-toolbar-end' }, dirtyMark, status, saveButton),
  );
  if (editor.message) status.textContent = editor.message;

  // ----- On the pages -----
  /** A pointer event's place on a page, in points from its top-left corner. */
  function pointOn(p, e) {
    const r = p.el.getBoundingClientRect();
    return [(e.clientX - r.left) / scale(), (e.clientY - r.top) / scale()];
  }
  const pageOf = (target) => pages.find((p) => p.el === target.closest?.('.pdf-page'));

  scroller.addEventListener('pointerdown', (e) => {
    const p = pageOf(e.target);
    if (!p || e.button !== 0 || busy) return;
    if (editor.tool === 'draw') {
      e.preventDefault();
      const [w, ht] = sizes[p.index];
      const line = document.createElementNS('http://www.w3.org/2000/svg', 'polyline');
      line.setAttribute('fill', 'none');
      line.setAttribute('stroke', editor.colors.draw);
      line.setAttribute('stroke-width', '2');
      line.setAttribute('stroke-linecap', 'round');
      line.setAttribute('stroke-linejoin', 'round');
      const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
      svg.setAttribute('class', 'pdf-ink-live');
      svg.setAttribute('viewBox', `0 0 ${w} ${ht}`);
      svg.append(line);
      p.el.append(svg);
      const points = [pointOn(p, e)];
      const draw = () => line.setAttribute('points', points.map((q) => q.join(',')).join(' '));
      draw();
      const move = (ev) => {
        const q = pointOn(p, ev);
        const last = points.at(-1);
        // Every half point is enough, and keeps the file small.
        if (Math.hypot(q[0] - last[0], q[1] - last[1]) >= 0.5) {
          points.push(q);
          draw();
        }
      };
      const up = async () => {
        removeEventListener('pointermove', move);
        removeEventListener('pointerup', up);
        const ok = await apply({ op: 'ink', page: p.index, strokes: [points.map((q) => q.map((v) => Math.round(v * 100) / 100))], color: editor.colors.draw, width: 2 });
        if (!ok) svg.remove();
      };
      addEventListener('pointermove', move);
      addEventListener('pointerup', up);
    } else if (editor.tool === 'redact') {
      e.preventDefault();
      const start = pointOn(p, e);
      const box = h('div', { class: 'pdf-redact-live', 'aria-hidden': 'true' });
      p.el.append(box);
      let rect = [...start, 0, 0];
      const move = (ev) => {
        const [x, y] = pointOn(p, ev);
        rect = [Math.min(x, start[0]), Math.min(y, start[1]), Math.abs(x - start[0]), Math.abs(y - start[1])];
        place(box, rect);
      };
      const up = async () => {
        removeEventListener('pointermove', move);
        removeEventListener('pointerup', up);
        if (rect[2] < 2 || rect[3] < 2) return box.remove();
        const done = await change('pdf_edit_content', { op: { op: 'redact', page: p.index, rect: rect.map(round) } }, t('Redacted. Save to make it final: until then Undo brings it back.'));
        if (!done) box.remove();
      };
      addEventListener('pointermove', move);
      addEventListener('pointerup', up);
    } else if (editor.tool === 'sign') {
      e.preventDefault();
      const signature = settings.get().signature;
      if (!signature) return drawSignature();
      const [x, y] = pointOn(p, e);
      const w = SIGNATURE_WIDTH;
      const hgt = w * signature.aspect;
      const strokes = signature.strokes.map((s) => s.map(([u, v]) => [round(x - w / 2 + u * w), round(y - hgt / 2 + v * w)]));
      apply({ op: 'ink', page: p.index, strokes, color: editor.colors.sign, width: 1.6 });
    } else if (editor.tool === 'fill' && !e.target.closest('.pdf-content > *, .pdf-inline')) {
      e.preventDefault();
      const [x, y] = pointOn(p, e);
      writeAt(p, [x, y - editor.fillSize * 0.7, Math.max(60, sizes[p.index][0] - x - 20), editor.fillSize]);
    } else if (editor.tool === 'note' || editor.tool === 'text') {
      e.preventDefault();
      const at = pointOn(p, e);
      const text = prompt(editor.tool === 'note' ? t('Note') : t('Text to write on the page'));
      if (!text?.trim()) return;
      if (editor.tool === 'note') apply({ op: 'note', page: p.index, at, text, color: editor.colors.note });
      else apply({ op: 'text', page: p.index, at, text, size: editor.textSize, color: editor.colors.text });
    }
  });

  // Highlight: whatever text is selected when the mouse is let go.
  scroller.addEventListener('pointerup', () => {
    if (editor.tool !== 'highlight') return;
    const sel = getSelection();
    if (!sel.rangeCount || sel.isCollapsed) return;
    const byPage = new Map();
    for (const r of sel.getRangeAt(0).getClientRects()) {
      if (r.width < 1 || r.height < 1) continue;
      const p = pages.find((q) => {
        const b = q.el.getBoundingClientRect();
        return r.left + r.width / 2 >= b.left && r.left + r.width / 2 <= b.right && r.top + r.height / 2 >= b.top && r.top + r.height / 2 <= b.bottom;
      });
      if (!p) continue;
      const b = p.el.getBoundingClientRect();
      const s = scale();
      const rect = [(r.left - b.left) / s, (r.top - b.top) / s, r.width / s, r.height / s].map((v) => Math.round(v * 100) / 100);
      if (!byPage.has(p.index)) byPage.set(p.index, []);
      byPage.get(p.index).push(rect);
    }
    sel.removeAllRanges();
    // One page at a time (a selection across pages becomes one annotation per page).
    (async () => {
      for (const [page, rects] of byPage) if (!(await apply({ op: 'highlight', page, rects, color: editor.colors.highlight }))) break;
    })();
  });

  // ----- Annotations and form fields, on each page's layers -----
  let annots = null;
  let fields = null;
  const layered = new Set();
  call('pdf_annotations', { doc })
    .then((a) => {
      annots = a;
      layered.forEach(decorate);
    })
    .catch(() => (annots = []));
  call('pdf_fields', { doc })
    .then((f) => {
      fields = f;
      layered.forEach(decorate);
    })
    .catch(() => (fields = []));

  const place = (el, [x, y, w, hgt]) => (el.style.cssText = `--x:${x};--y:${y};--w:${w};--h:${hgt}`);

  /** Erase targets and note tips over a page; inputs over its form fields. */
  function decorate(p) {
    p.el.querySelector('.pdf-annots')?.remove();
    p.el.querySelector('.pdf-fields')?.remove();
    const marks = (annots?.[p.index] ?? []).map((a) => {
      const label = a.contents ? `${a.contents}` : t('Annotation');
      const b = h('button', { type: 'button', class: 'pdf-annot', title: label, 'aria-label': t('Erase: {what}', { what: label }), 'data-kind': a.subtype });
      place(b, a.rect);
      b.addEventListener('click', () => editor.tool === 'erase' && apply({ op: 'removeAnnot', page: p.index, index: a.index }));
      return b;
    });
    const inputs = (fields ?? []).flatMap((f) =>
      f.widgets
        .filter((w) => w.page === p.index)
        .map((w) => {
          let input;
          if (f.kind === 'check' || f.kind === 'radio') {
            input = h('input', { type: f.kind === 'check' ? 'checkbox' : 'radio', name: `pdf-${doc}-${f.name}`, checked: f.kind === 'check' ? f.value !== '' && f.value !== 'Off' : f.value === w.on });
            input.addEventListener('change', () => apply({ op: 'fill', name: f.name, value: f.kind === 'check' ? String(input.checked) : w.on }));
          } else if (f.kind === 'choice' && f.options.length) {
            input = h('select', {}, h('option', { value: '' }, ''), f.options.map((o) => h('option', { value: o, selected: o === f.value }, o)));
            input.addEventListener('change', () => apply({ op: 'fill', name: f.name, value: input.value }));
          } else {
            input = h(f.multiline ? 'textarea' : 'input', { type: 'text', value: f.value, spellcheck: 'false' });
            input.value = f.value;
            input.addEventListener('change', () => input.value !== f.value && apply({ op: 'fill', name: f.name, value: input.value }));
          }
          input.className = 'pdf-field';
          input.title = f.name;
          input.setAttribute('aria-label', f.name);
          place(input, w.rect);
          return input;
        }),
    );
    p.el.append(h('div', { class: 'pdf-annots' }, marks), h('div', { class: 'pdf-fields' }, inputs));
    p.el.querySelector('.pdf-content')?.remove();
    if (CONTENT_TOOLS.has(editor.tool)) {
      const content = contentOf(p);
      if (content) p.el.append(h('div', { class: 'pdf-content' }, contentLayer(p, content)));
    }
  }

  // ----- What the page itself draws: lines to edit, pictures to move, blanks to fill -----
  const contents = new Map();
  /** The page's content if it's here; otherwise asks for it and decorates the page when it comes. */
  function contentOf(p) {
    const known = contents.get(p.index);
    if (known !== undefined) return known;
    contents.set(p.index, null);
    call('pdf_page_content', { doc, page: p.index })
      .then((c) => contents.set(p.index, c))
      .catch((error) => {
        contents.set(p.index, { lines: [], pictures: [], blanks: [] });
        status.textContent = problem(error);
      })
      .finally(() => layered.has(p) && CONTENT_TOOLS.has(editor.tool) && decorate(p));
    return null;
  }

  function contentLayer(p, content) {
    if (editor.tool === 'editText') {
      return content.lines.map((line) => {
        const b = h('button', { type: 'button', class: 'pdf-line', title: line.text, 'aria-label': t('Edit: {what}', { what: line.text }) });
        place(b, line.rect);
        b.addEventListener('click', () => editLine(p, line, b));
        return b;
      });
    }
    if (editor.tool === 'fill') {
      return content.blanks.map((blank) => {
        const b = h('button', { type: 'button', class: 'pdf-blank', 'data-kind': blank.kind, 'aria-label': blank.kind === 'box' ? t('Mark this box') : t('Write here') });
        place(b, blank.rect);
        b.addEventListener('click', () => (blank.kind === 'box' ? markBox(p, blank.rect) : writeAt(p, blank.rect)));
        return b;
      });
    }
    return content.pictures.map((pic) => pictureBox(p, pic));
  }

  /** A text box over `rect` (points on the page); `done(text)` gets what was typed. */
  function inlineInput(p, rect, { value = '', size, family = 'sans-serif', done, closed }) {
    scroller.querySelector('.pdf-inline')?.blur();
    const input = h('input', { type: 'text', class: 'pdf-inline', spellcheck: 'false', 'aria-label': t('Text') });
    input.value = value;
    place(input, rect);
    input.style.setProperty('--size', size);
    input.style.fontFamily = family;
    let finished = false;
    const finish = (keep) => {
      if (finished) return;
      finished = true;
      input.remove();
      closed?.();
      if (keep && input.value !== value) done(input.value);
    };
    input.addEventListener('keydown', (e) => {
      e.stopPropagation();
      if (e.key === 'Enter') finish(true);
      else if (e.key === 'Escape') finish(false);
    });
    input.addEventListener('blur', () => finish(true));
    p.el.append(input);
    input.focus();
    input.select();
  }

  function editLine(p, line, button) {
    button.hidden = true;
    const [x, y, w, hgt] = line.rect;
    inlineInput(p, [x - 2, y, Math.max(w + 40, 80), hgt], {
      value: line.text,
      size: line.size,
      family: familyOf(line.font),
      done: (text) => change('pdf_edit_content', { op: { op: 'replaceText', page: p.index, line: line.index, text } }),
      closed: () => (button.hidden = false),
    });
  }

  /** Writes on the page at `rect`: the text sits on its bottom, as on a printed line. */
  function writeAt(p, [x, y, w, hgt]) {
    const size = editor.fillSize;
    const baseline = y + hgt;
    inlineInput(p, [x, baseline - size * 1.3, Math.max(w, 60), size * 1.3], {
      size,
      done: (text) => text.trim() && apply({ op: 'text', page: p.index, at: [round(x - 2), round(baseline - 3 - size * 0.8)], text, size, color: editor.colors.fill }),
    });
  }

  function insertDate() {
    const input = scroller.querySelector('.pdf-inline');
    if (!input) {
      status.textContent = t('Click a blank first; the date goes in it.');
      return;
    }
    const date = new Date().toLocaleDateString(document.documentElement.lang || undefined);
    input.setRangeText(date, input.selectionStart, input.selectionEnd, 'end');
    input.focus();
  }

  /** A ✓ or ✗ drawn in a box. */
  function markBox(p, [x, y, w, hgt]) {
    const at = (fx, fy) => [round(x + fx * w), round(y + fy * hgt)];
    const strokes = editor.mark === 'cross' ? [[at(0.22, 0.22), at(0.78, 0.78)], [at(0.78, 0.22), at(0.22, 0.78)]] : [[at(0.2, 0.55), at(0.42, 0.78), at(0.82, 0.22)]];
    apply({ op: 'ink', page: p.index, strokes, color: editor.colors.fill, width: round(Math.max(1, w * 0.12)) });
  }

  // Pictures: drag to move, drag the corner to size (keeping the shape unless Shift is held).
  function pictureBox(p, pic) {
    const chosen = editor.picture?.page === p.index && editor.picture.index === pic.index;
    const box = h('div', { class: 'pdf-picture', tabindex: '0', role: 'button', 'aria-pressed': String(chosen), 'aria-label': t('Picture {n}', { n: pic.index + 1 }) }, h('span', { class: 'pdf-picture-handle', 'aria-hidden': 'true' }));
    place(box, pic.rect);
    const select = () => {
      editor.picture = { page: p.index, index: pic.index };
      for (const other of scroller.querySelectorAll('.pdf-picture')) other.setAttribute('aria-pressed', String(other === box));
      replaceButton.disabled = deletePictureButton.disabled = false;
    };
    box.addEventListener('focus', select);
    box.addEventListener('keydown', (e) => {
      if (e.key === 'Delete' || e.key === 'Backspace') {
        e.preventDefault();
        deletePicture();
      }
    });
    box.addEventListener('pointerdown', (e) => {
      if (e.button !== 0 || busy) return;
      e.preventDefault();
      e.stopPropagation();
      box.focus();
      select();
      const resize = e.target.classList.contains('pdf-picture-handle');
      const start = pointOn(p, e);
      let rect = [...pic.rect];
      const move = (ev) => {
        const [px, py] = pointOn(p, ev);
        const [dx, dy] = [px - start[0], py - start[1]];
        const [x, y, w, hgt] = pic.rect;
        if (!resize) rect = [x + dx, y + dy, w, hgt];
        else {
          const nw = Math.max(4, w + dx);
          rect = [x, y, nw, ev.shiftKey ? Math.max(4, hgt + dy) : nw * (hgt / w)];
        }
        place(box, rect);
      };
      const up = async () => {
        removeEventListener('pointermove', move);
        removeEventListener('pointerup', up);
        if (!rect.some((v, i) => Math.abs(v - pic.rect[i]) > 0.5)) return;
        if (!(await change('pdf_edit_content', { op: { op: 'moveImage', page: p.index, index: pic.index, rect: rect.map(round) } }))) place(box, pic.rect);
      };
      addEventListener('pointermove', move);
      addEventListener('pointerup', up);
    });
    return box;
  }

  function replacePicture() {
    const pic = editor.picture;
    if (pic) change('pdf_replace_image', { page: pic.page, index: pic.index });
  }
  function deletePicture() {
    const pic = editor.picture;
    if (!pic) return;
    editor.picture = null;
    change('pdf_edit_content', { op: { op: 'deleteImage', page: pic.page, index: pic.index } });
  }
  function layer(p) {
    layered.add(p);
    decorate(p);
  }

  // ----- Signature, compress, OCR -----
  /** A pad to draw the signature on; kept in the settings for the next time. */
  function drawSignature() {
    const W = 480;
    const H = 180;
    const canvas = h('canvas', { class: 'pdf-sign-pad', width: String(W * devicePixelRatio), height: String(H * devicePixelRatio), 'aria-label': t('Draw your signature here') });
    canvas.style.aspectRatio = `${W} / ${H}`;
    const ctx = canvas.getContext('2d');
    ctx.scale(devicePixelRatio, devicePixelRatio);
    ctx.lineWidth = 2.5;
    ctx.lineCap = ctx.lineJoin = 'round';
    ctx.strokeStyle = editor.colors.sign;
    /** @type {[number, number][][]} */
    let strokes = [];
    const at = (e) => {
      const r = canvas.getBoundingClientRect();
      return [((e.clientX - r.left) / r.width) * W, ((e.clientY - r.top) / r.height) * H];
    };
    canvas.addEventListener('pointerdown', (e) => {
      e.preventDefault();
      canvas.setPointerCapture(e.pointerId);
      strokes.push([at(e)]);
    });
    canvas.addEventListener('pointermove', (e) => {
      const stroke = canvas.hasPointerCapture(e.pointerId) && strokes.at(-1);
      if (!stroke) return;
      const q = at(e);
      const last = stroke.at(-1);
      if (Math.hypot(q[0] - last[0], q[1] - last[1]) < 1.5) return;
      ctx.beginPath();
      ctx.moveTo(...last);
      ctx.lineTo(...q);
      ctx.stroke();
      stroke.push(q);
    });
    const use = () => {
      const points = strokes.flat();
      if (points.length < 2) return dialog.close();
      const xs = points.map((q) => q[0]);
      const ys = points.map((q) => q[1]);
      const [x0, y0] = [Math.min(...xs), Math.min(...ys)];
      const w = Math.max(1, Math.max(...xs) - x0);
      const hgt = Math.max(1, Math.max(...ys) - y0);
      // In a box 1 wide, its height in proportion.
      const norm = strokes.map((s) => (s.length === 1 ? [s[0], [s[0][0] + 0.5, s[0][1]]] : s).map(([x, y]) => [Math.round(((x - x0) / w) * 1000) / 1000, Math.round(((y - y0) / w) * 1000) / 1000]));
      settings.update({ signature: { strokes: norm, aspect: hgt / w } });
      dialog.close();
      status.textContent = t('Click where the signature goes.');
    };
    const dialog = h(
      'dialog',
      { class: 'img-dialog pdf-sign-dialog', 'aria-labelledby': 'pdf-sign-title' },
      h('h2', { id: 'pdf-sign-title' }, t('Your signature')),
      h('p', { class: 'hint' }, t('Draw it with the mouse, a pen or a finger. Jay-E keeps it for next time.')),
      canvas,
      h(
        'div',
        { class: 'img-dialog-actions' },
        h('button', { type: 'button', class: 'btn', onClick: use }, t('Use it')),
        h('button', { type: 'button', class: 'btn btn-secondary', onClick: () => ((strokes = []), ctx.clearRect(0, 0, W, H)) }, t('Clear')),
        h('button', { type: 'button', class: 'btn btn-secondary', onClick: () => dialog.close() }, t('Cancel')),
      ),
    );
    dialog.addEventListener('close', () => dialog.remove());
    document.body.append(dialog);
    dialog.showModal();
  }

  async function compress() {
    if (busy) return;
    busy = true;
    status.textContent = t('Compressing…');
    try {
      const done = await call('pdf_compress', { doc });
      editor.dirty = done.edited.dirty;
      editor.canUndo = done.edited.can_undo;
      editor.message = t('{before} → {after}. Save to keep it.', { before: formatSize(done.before), after: formatSize(done.after) });
      busy = false;
      rebuild(done.edited);
    } catch (error) {
      status.textContent = problem(error);
    } finally {
      busy = false;
    }
  }

  async function makeSearchable() {
    if (busy) return;
    busy = true;
    status.textContent = t('Reading the pages…');
    try {
      const { Channel } = await import('@tauri-apps/api/core');
      const progress = new Channel();
      progress.onmessage = ([done, all]) => (status.textContent = t('Reading page {done} of {all}…', { done, all }));
      const done = await call('pdf_make_searchable', { doc, progress });
      editor.dirty = done.edited.dirty;
      editor.canUndo = done.edited.can_undo;
      editor.message = t('Text found on {n} page(s). Save to keep it.', { n: done.pages });
      busy = false;
      rebuild(done.edited);
    } catch (error) {
      status.textContent = problem(error);
    } finally {
      busy = false;
    }
  }

  // ----- The page organizer -----
  const organizer = h('div', { class: 'pdf-organizer', hidden: !editor.organizer, role: 'region', 'aria-label': t('Pages') });
  function selectedList() {
    return [...editor.selected].filter((i) => i < sizes.length).sort((a, b) => a - b);
  }
  function orgButton(icon, label, onClick, extra = {}) {
    return h('button', { type: 'button', class: 'btn btn-secondary', onClick, ...extra }, h('span', { class: 'btn-icon', html: icons[icon] }), label);
  }
  /** The order with the selected pages moved to just before `target` (an index in the current order). */
  function moveTo(target) {
    const chosen = selectedList();
    if (!chosen.length) return;
    const rest = sizes.map((_, i) => i).filter((i) => !editor.selected.has(i));
    const at = rest.filter((i) => i < target).length;
    const order = [...rest.slice(0, at), ...chosen, ...rest.slice(at)];
    if (order.every((v, i) => v === i)) return;
    editor.selected = new Set(chosen.map((_, k) => at + k));
    apply({ op: 'order', order });
  }
  function buildOrganizer() {
    const count = selectedList().length;
    const items = sizes.map(([w, hgt], i) => {
      const canvas = h('canvas', { 'aria-hidden': 'true' });
      canvas.style.aspectRatio = `${w} / ${hgt}`;
      const item = h(
        'button',
        { type: 'button', class: 'pdf-org-page', draggable: 'true', 'aria-pressed': String(editor.selected.has(i)), 'aria-label': t('Page {n}', { n: i + 1 }), 'data-index': String(i) },
        canvas,
        h('span', {}, String(i + 1)),
      );
      fetchPage(doc, i, ORGANIZER_WIDTH * devicePixelRatio, { thumb: true })
        .then((b) => {
          if (!b) return;
          canvas.width = b.width;
          canvas.height = b.height;
          canvas.getContext('2d').drawImage(b, 0, 0);
          b.close();
        })
        .catch(() => {});
      item.addEventListener('click', (e) => {
        if (e.shiftKey && editor.anchor != null) {
          const [a, b] = [Math.min(editor.anchor, i), Math.max(editor.anchor, i)];
          for (let k = a; k <= b; k++) editor.selected.add(k);
        } else if (e.ctrlKey || e.metaKey) {
          editor.selected.has(i) ? editor.selected.delete(i) : editor.selected.add(i);
        } else {
          editor.selected = new Set([i]);
        }
        editor.anchor = i;
        buildOrganizer();
      });
      // Drag a page (or the selected ones) onto another to put them before it.
      item.addEventListener('dragstart', (e) => {
        if (!editor.selected.has(i)) editor.selected = new Set([i]);
        e.dataTransfer.effectAllowed = 'move';
        e.dataTransfer.setData('text/plain', String(i));
      });
      item.addEventListener('dragover', (e) => {
        e.preventDefault();
        item.classList.add('is-drop');
      });
      item.addEventListener('dragleave', () => item.classList.remove('is-drop'));
      item.addEventListener('drop', (e) => {
        e.preventDefault();
        item.classList.remove('is-drop');
        moveTo(i);
      });
      return item;
    });
    const end = h('div', { class: 'pdf-org-end', 'aria-hidden': 'true' });
    end.addEventListener('dragover', (e) => e.preventDefault());
    end.addEventListener('drop', (e) => {
      e.preventDefault();
      moveTo(sizes.length);
    });
    const insertAt = count ? selectedList().at(-1) + 1 : sizes.length;
    organizer.replaceChildren(
      h(
        'div',
        { class: 'pdf-org-bar' },
        h('span', { class: 'hint' }, count ? t('{n} selected', { n: count }) : t('Click a page to select it; drag to move.')),
        orgButton('rotateLeft', t('Turn left'), () => apply({ op: 'rotate', pages: selectedList(), by: -90 }), { disabled: !count }),
        orgButton('rotateRight', t('Turn right'), () => apply({ op: 'rotate', pages: selectedList(), by: 90 }), { disabled: !count }),
        orgButton('close', t('Delete'), () => {
          const chosen = selectedList();
          if (!chosen.length || !confirm(t('Delete {n} page(s)?', { n: chosen.length }))) return;
          editor.selected = new Set();
          apply({ op: 'delete', pages: chosen });
        }, { disabled: !count }),
        orgButton('plus', t('Insert a PDF…'), () => change('pdf_edit_insert', { at: insertAt })),
        orgButton('file', t('Save pages as…'), async () => {
          try {
            const where = await call('pdf_edit_extract', { doc, pages: selectedList() });
            if (where) status.textContent = t('Saved as {path}', { path: where });
          } catch (error) {
            status.textContent = problem(error);
          }
        }, { disabled: !count }),
        h('button', { type: 'button', class: 'btn', onClick: () => toggleOrganizer(false) }, t('Done')),
      ),
      h('div', { class: 'pdf-org-grid' }, items, end),
    );
  }
  function toggleOrganizer(show = organizer.hidden) {
    editor.organizer = show;
    organizer.hidden = !show;
    pagesButton.setAttribute('aria-pressed', String(show));
    if (show) buildOrganizer();
  }
  if (editor.organizer) buildOrganizer();

  setTool(editor.tool);
  return {
    bar,
    organizer,
    layer,
    /** Ctrl+S saves, Ctrl+Z undoes (outside form fields, which undo their own typing). */
    key(e) {
      const k = e.key.toLowerCase();
      if (k === 's') {
        e.preventDefault();
        save();
        return true;
      }
      if (k === 'z' && !e.target.closest?.('input, textarea, select')) {
        e.preventDefault();
        undo();
        return true;
      }
      return false;
    },
  };
}
