// Editing in the PDF viewer (stage 7): the pages (an organizer to reorder, turn, delete, insert and
// take pages out), annotations (highlight, draw, note, text, erase) and form fields.
//
// Every change goes to Rust (src-tauri/src/pdf_edit.rs → jay_docs::pdf_edit), which makes the new
// PDF and opens it as a new document; the viewer is rebuilt around it (`rebuild`) where it was. So
// what's shown is always the real file, annotations drawn by the same renderer as the rest.
import { h } from './dom.js';
import { icons } from './icons.js';
import { t } from '../core/i18n.js';
import { fetchPage } from '../core/pdf.js';

const call = async (cmd, args) => (await import('@tauri-apps/api/core')).invoke(cmd, args);
const COLORS = { highlight: '#ffd400', draw: '#e53935', note: '#ffd54f', text: '#1a73e8' };
const ORGANIZER_WIDTH = 150;

/** The error a change came back with, in words. */
function problem(error) {
  if (error?.kind === 'encrypted') return t("Password-protected PDFs can't be edited.");
  if (error?.kind === 'empty') return t('A PDF needs at least one page.');
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
  editor.selected ??= new Set();
  let busy = false;

  const status = h('span', { class: 'hint doc-status', role: 'status' });
  const dirtyMark = h('span', { class: 'editor-dirty', hidden: !editor.dirty, title: t('Not saved'), 'aria-label': t('Not saved') }, '●');

  /** Sends a change; on success the viewer is rebuilt on the new document. */
  async function change(cmd, args) {
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
      editor.message = '';
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
  ];
  const colorInput = h('input', { type: 'color', class: 'pdf-color', title: t('Color'), 'aria-label': t('Color') });
  colorInput.addEventListener('input', () => (editor.colors[editor.tool] = colorInput.value));
  const sizeSelect = h(
    'select',
    { class: 'input', title: t('Text size'), 'aria-label': t('Text size'), onChange: (e) => (editor.textSize = Number(e.target.value)) },
    [10, 12, 14, 18, 24, 32].map((s) => h('option', { value: String(s), selected: s === editor.textSize }, `${s} pt`)),
  );
  function setTool(tool) {
    editor.tool = tool;
    for (const b of tools) b.setAttribute('aria-pressed', String(b.dataset.tool === tool));
    scroller.dataset.tool = tool;
    colorInput.hidden = !COLORS[tool];
    if (COLORS[tool]) colorInput.value = editor.colors[tool];
    sizeSelect.hidden = tool !== 'text';
  }
  const pagesButton = h('button', { type: 'button', class: 'btn btn-secondary', 'aria-pressed': String(!!editor.organizer), onClick: () => toggleOrganizer() }, h('span', { class: 'btn-icon', html: icons.pages }), t('Pages'));
  const undoButton = h('button', { type: 'button', class: 'icon-btn', title: t('Undo (Ctrl+Z)'), 'aria-label': t('Undo (Ctrl+Z)'), html: icons.undo, disabled: !editor.canUndo, onClick: () => undo() });
  const saveButton = h('button', { type: 'button', class: 'btn', disabled: !editor.dirty, onClick: () => save() }, t('Save'));
  const undo = () => editor.canUndo && change('pdf_edit_undo', {});
  const bar = h(
    'div',
    { class: 'doc-toolbar pdf-edit-bar', role: 'toolbar', 'aria-label': t('PDF editing') },
    pagesButton,
    h('span', { class: 'dx-sep', 'aria-hidden': 'true' }),
    h('span', { class: 'img-tool-group' }, tools),
    colorInput,
    sizeSelect,
    h('span', { class: 'dx-sep', 'aria-hidden': 'true' }),
    undoButton,
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
  }
  function layer(p) {
    layered.add(p);
    decorate(p);
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
