// The document viewer (stage 4): DOCX, ODT and RTF read in Rust (jay_docs::doc) and shown as one
// continuous A4 sheet; explicit page breaks split it. Export to DOCX, Markdown, HTML, text, and to
// PDF through the print dialog.
import { h } from './dom.js';
import { icons } from './icons.js';
import { t } from '../core/i18n.js';
import { openLink } from '../core/pdf.js';

const call = async (cmd, args) => (await import('@tauri-apps/api/core')).invoke(cmd, args);
const SHEET_PX = 794; // A4 width at 96 dpi; keep in sync with .doc-sheet
const ZOOMS = [0.5, 0.75, 1, 1.25, 1.5, 2];

export async function createDocViewer(file) {
  let opened;
  try {
    opened = await call('doc_open', { path: file.path, format: file.format });
  } catch (error) {
    const message = error?.kind === 'toobig' ? t('This document is too big.') : error?.kind === 'unsupported' ? t("Jay-E doesn't open this kind of file.") : (error?.message ?? String(error));
    return h('div', { class: 'view-notice' }, h('h2', {}, file.name), h('p', {}, t("Couldn't open it: {error}", { error: message })));
  }
  const { id, html } = opened;

  const content = h('article', { class: 'doc-content', 'aria-label': file.name });
  // Built by jay_docs::doc::html, which escapes every text and keeps only http(s)/mailto links.
  content.innerHTML = html;
  const sheet = h('div', { class: 'doc-sheet' }, content);
  const stage = h('div', { class: 'doc-stage', tabindex: '0' }, sheet);
  content.addEventListener('click', (e) => {
    const a = e.target.closest('a[href]');
    if (!a) return;
    e.preventDefault();
    if (/^(https?:|mailto:)/i.test(a.getAttribute('href'))) openLink(a.getAttribute('href')).catch(() => {});
  });

  // ----- Zoom -----
  let zoom = 'width';
  const zoomLabel = h('span', { class: 'hint img-zoom-label' });
  function layout() {
    const fit = (stage.clientWidth - 32) / SHEET_PX;
    const z = zoom === 'width' ? Math.max(Math.min(fit, 1.5), 0.3) : zoom;
    sheet.style.zoom = String(z);
    zoomLabel.textContent = `${Math.round(z * 100)}%`;
  }
  const step = (dir) => {
    const now = parseFloat(sheet.style.zoom) || 1;
    const next = dir > 0 ? ZOOMS.find((z) => z > now + 0.01) : [...ZOOMS].reverse().find((z) => z < now - 0.01);
    if (next) {
      zoom = next;
      layout();
    }
  };

  // ----- Export -----
  const status = h('span', { class: 'hint doc-status', role: 'status' });
  const format = h(
    'select',
    { class: 'input', 'aria-label': t('Export as') },
    [
      ['pdf', 'PDF'],
      ['docx', 'DOCX'],
      ['markdown', 'Markdown'],
      ['html', 'HTML'],
      ['txt', t('Plain text')],
    ].map(([value, label]) => h('option', { value }, label)),
  );
  async function exportIt() {
    if (format.value === 'pdf') return printIt();
    status.textContent = '';
    try {
      const where = await call('doc_export', { id, format: format.value });
      if (where) status.textContent = t('Saved as {path}', { path: where });
    } catch (error) {
      status.textContent = t("Couldn't export: {error}", { error: error?.message ?? error?.kind ?? String(error) });
    }
  }
  function printIt() {
    const printRoot = h('div', { id: 'print-root', class: 'doc-content' });
    printRoot.innerHTML = content.innerHTML;
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

  const iconButton = (icon, label, onClick) => h('button', { type: 'button', class: 'icon-btn', title: label, 'aria-label': label, html: icon, onClick });
  const toolbar = h(
    'div',
    { class: 'doc-toolbar', role: 'toolbar', 'aria-label': t('Document tools') },
    h(
      'span',
      { class: 'img-tool-group' },
      iconButton(icons.minus, t('Zoom out'), () => step(-1)),
      h('button', { type: 'button', class: 'btn btn-secondary', onClick: () => ((zoom = 'width'), layout()) }, t('Fit width')),
      h('button', { type: 'button', class: 'btn btn-secondary', onClick: () => ((zoom = 1), layout()) }, '100%'),
      iconButton(icons.plus, t('Zoom in'), () => step(1)),
      zoomLabel,
    ),
    status,
    h('span', { class: 'img-tool-group doc-export' }, format, h('button', { type: 'button', class: 'btn', onClick: exportIt }, t('Export'))),
  );

  const root = h('div', { class: 'doc-viewer' }, toolbar, stage);
  root.addEventListener('keydown', (e) => {
    if (!e.ctrlKey || e.altKey) return;
    const actions = { '=': () => step(1), '+': () => step(1), '-': () => step(-1), 0: () => ((zoom = 'width'), layout()), p: printIt };
    const action = actions[e.key];
    if (action) {
      e.preventDefault();
      action();
    }
  });
  new ResizeObserver(() => stage.isConnected && stage.clientWidth && layout()).observe(stage);
  root.dispose = () => call('doc_close', { id }).catch(() => {});
  return root;
}
