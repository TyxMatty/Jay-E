// The Files section: a bar with the open files (one tab each) and the viewer of the chosen one.
// Files come from the Open button (Ctrl+O), a drop on the window or the command line.
import { h } from './dom.js';
import { icons } from './icons.js';
import { createViewer } from './viewers.js';
import { isTauri, pickFiles, startupFiles, onFilesOpened, onDragOver, onCloseRequested } from '../core/files.js';
import { t } from '../core/i18n.js';

/** The open files, for viewers that act on several (image conversion). */
let openDocs = [];
export const openImageFiles = () => openDocs.filter((d) => d.file.kind === 'image' && !d.file.error).map((d) => d.file);

export function createFilesPanel() {
  /** @type {Array<{ id: number, file: object, viewer?: Promise<HTMLElement> }>} */
  const docs = (openDocs = []);
  let current = null;
  let nextId = 1;

  const openButton = h('button', { type: 'button', class: 'btn', disabled: !isTauri, onClick: () => open() }, h('span', { class: 'btn-icon', html: icons.folder }), t('Open'));
  const docBar = h('div', { class: 'doc-bar', role: 'tablist', 'aria-label': t('Open files') });
  const view = h('div', { class: 'doc-view' });
  const dropHint = h('div', { class: 'drop-hint', hidden: true, 'aria-hidden': 'true' }, t('Drop to open'));

  const empty = h(
    'div',
    { class: 'view-notice view-empty' },
    h('span', { class: 'empty-icon', html: icons.file, 'aria-hidden': 'true' }),
    h('h2', {}, t('Open a file or drop it here')),
    h('p', { class: 'hint' }, isTauri ? t('PDF, Markdown, text, images, video and audio today; DOCX next.') : t('Only in the app: the browser preview opens no files.')),
  );

  // A newer pick replaces an older one still loading.
  let renderToken = 0;
  async function show(id) {
    current = id;
    const token = ++renderToken;
    for (const tab of docBar.querySelectorAll('[role="tab"]')) {
      const on = Number(tab.dataset.id) === id;
      tab.setAttribute('aria-selected', String(on));
      tab.tabIndex = on ? 0 : -1;
    }
    // A hidden tab keeps its viewer (and where it was), but its video or audio stops.
    for (const media of view.querySelectorAll('video, audio')) media.pause();
    const doc = docs.find((d) => d.id === id);
    if (!doc) return view.replaceChildren(empty);
    doc.viewer ??= createViewer(doc.file);
    const viewer = (doc.el = await doc.viewer);
    if (token === renderToken) view.replaceChildren(viewer);
  }

  function renderBar() {
    docBar.replaceChildren(
      ...docs.map(({ id, file }) =>
        h(
          'div',
          { class: 'doc-tab' },
          h('button', { type: 'button', role: 'tab', 'data-id': id, title: file.path, onClick: () => show(id) }, h('span', { class: 'doc-tab-name' }, file.name)),
          h('button', { type: 'button', class: 'doc-tab-close', 'aria-label': t('Close {name}', { name: file.name }), title: t('Close'), html: icons.close, onClick: () => close(id) }),
        ),
      ),
    );
    docBar.hidden = docs.length === 0;
  }

  function add(files) {
    if (!files?.length) return;
    let last = null;
    for (const file of files) {
      // The same file again goes to its tab.
      const existing = docs.find((d) => d.file.path === file.path);
      last = existing?.id ?? nextId++;
      if (!existing) docs.push({ id: last, file });
    }
    renderBar();
    show(last);
  }

  /** Unsaved edits are never dropped without asking. */
  const confirmDiscard = (dirty) => !dirty.length || confirm(t('Unsaved changes in {names}. Close anyway?', { names: dirty.map((d) => d.file.name).join(', ') }));

  function close(id) {
    const index = docs.findIndex((d) => d.id === id);
    if (index < 0 || !confirmDiscard(docs.filter((d) => d.id === id && d.el?.isDirty?.()))) return;
    const [doc] = docs.splice(index, 1);
    doc.viewer?.then((viewer) => viewer.dispose?.());
    renderBar();
    if (current === id) show(docs[Math.min(index, docs.length - 1)]?.id ?? null);
    docBar.querySelector('[aria-selected="true"]')?.focus();
  }

  async function open() {
    add(await pickFiles());
  }

  // Arrow keys move between the open files (the tabs pattern of ui/tabs.js).
  docBar.addEventListener('keydown', (e) => {
    const steps = { ArrowRight: 1, ArrowLeft: -1 };
    if (!(e.key in steps) || !docs.length) return;
    e.preventDefault();
    const index = docs.findIndex((d) => d.id === current);
    const next = docs[(index + steps[e.key] + docs.length) % docs.length];
    show(next.id).then(() => docBar.querySelector(`[data-id="${next.id}"]`)?.focus());
  });

  document.addEventListener('keydown', (e) => {
    if (!e.ctrlKey || e.altKey || e.shiftKey) return;
    if (e.key === 'o' && isTauri) {
      e.preventDefault();
      open();
    } else if (e.key === 'w' && current != null) {
      e.preventDefault();
      close(current);
    }
  });

  onCloseRequested((event) => {
    if (!confirmDiscard(docs.filter((d) => d.el?.isDirty?.()))) event.preventDefault();
  });
  onFilesOpened(add);
  onDragOver((over) => (dropHint.hidden = !over));
  startupFiles().then(add);

  renderBar();
  view.replaceChildren(empty);
  return h('section', { class: 'app-panel files-panel' }, h('div', { class: 'files-head' }, docBar, openButton), view, dropHint);
}
