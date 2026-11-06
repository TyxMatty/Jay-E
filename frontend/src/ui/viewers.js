// One viewer per kind of file (jay_docs::Kind). Images, video, audio and text use what WebView2
// already does fast; PDF is drawn by Rust (pdf-viewer.js). The other kinds say which stage brings
// their viewer (docs/ARQUITETURA.md, section 8).
import { h } from './dom.js';
import { fileUrl, decodeText, formatSize } from '../core/files.js';
import { createPdfViewer } from './pdf-viewer.js';
import { t } from '../core/i18n.js';

/** Larger text files wait for the editor of stage 2, which won't load them whole. */
const MAX_TEXT_BYTES = 20 * 1024 * 1024;

/** The stage that brings the viewer of each kind not shown yet. */
const STAGE = { markdown: 2, document: 4, latex: 6 };

const notice = (file, ...lines) =>
  h(
    'div',
    { class: 'view-notice' },
    h('h2', {}, file.name),
    h('p', { class: 'hint' }, [file.format.toUpperCase(), formatSize(file.size)].filter(Boolean).join(' · ')),
    lines.map((line) => h('p', {}, line)),
  );

async function textView(file, url) {
  if (file.size > MAX_TEXT_BYTES) return notice(file, t('This file is too big to show as text ({size} max).', { size: formatSize(MAX_TEXT_BYTES) }));
  const response = await fetch(url);
  if (!response.ok) throw new Error(`HTTP ${response.status}`);
  const pre = h('pre', { class: 'view-text', tabindex: '0', 'aria-label': file.name }, decodeText(await response.arrayBuffer()));
  const stage = STAGE[file.kind];
  return stage ? h('div', { class: 'view-stack' }, h('p', { class: 'hint view-banner' }, t('Showing the source. The formatted view arrives in stage {stage}.', { stage })), pre) : pre;
}

function imageView(file, url) {
  // Fits the window; a click shows it at its real size (and back).
  const img = h('img', { class: 'view-image', src: url, alt: file.name, draggable: 'false' });
  const box = h('button', { type: 'button', class: 'view-image-box', title: t('Click to switch between fit and real size') }, img);
  box.addEventListener('click', () => box.classList.toggle('is-actual'));
  return box;
}

/** Builds the viewer of `file`. Never throws: a failure shows on the page. */
export async function createViewer(file) {
  if (file.error) return notice(file, t("Couldn't open it: {error}", { error: file.error }));
  try {
    if (file.kind === 'pdf') return await createPdfViewer(file);
    const url = await fileUrl(file);
    switch (file.kind) {
      case 'image':
        return imageView(file, url);
      case 'video':
        return h('video', { class: 'view-media', src: url, controls: true, preload: 'metadata', 'aria-label': file.name });
      case 'audio':
        return h('div', { class: 'view-notice' }, h('h2', {}, file.name), h('audio', { src: url, controls: true, preload: 'metadata', 'aria-label': file.name }));
      case 'text':
      case 'markdown':
      case 'latex':
        return await textView(file, url);
      case 'unknown':
        return notice(file, t("Jay-E doesn't open this kind of file."));
      default:
        return notice(file, t('The viewer for this kind of file arrives in stage {stage}.', { stage: STAGE[file.kind] }));
    }
  } catch (error) {
    return notice(file, t("Couldn't open it: {error}", { error: error?.message ?? String(error) }));
  }
}
