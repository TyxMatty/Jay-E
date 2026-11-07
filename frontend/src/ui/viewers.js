// One viewer per kind of file (jay_docs::Kind): PDF drawn by Rust (pdf-viewer.js), documents read
// by Rust (doc-viewer.js), text, Markdown and LaTeX in the editor (text-editor.js), images in the
// image editor (image-viewer.js); video and audio use what WebView2 already does fast.
import { h } from './dom.js';
import { fileUrl, formatSize } from '../core/files.js';
import { t } from '../core/i18n.js';


const notice = (file, ...lines) =>
  h(
    'div',
    { class: 'view-notice' },
    h('h2', {}, file.name),
    h('p', { class: 'hint' }, [file.format.toUpperCase(), formatSize(file.size)].filter(Boolean).join(' · ')),
    lines.map((line) => h('p', {}, line)),
  );

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
    // Loaded when first needed, so starting the app doesn't parse the editor.
    if (file.kind === 'pdf') return await (await import('./pdf-viewer.js')).createPdfViewer(file);
    if (file.kind === 'document') return await (await import('./doc-viewer.js')).createDocViewer(file);
    if (['text', 'markdown', 'latex'].includes(file.kind)) return await (await import('./text-editor.js')).createTextEditor(file);
    const url = await fileUrl(file);
    // Rust-decoded editor; formats it can't decode (AVIF, SVG) are shown by the WebView instead.
    if (file.kind === 'image') return await (await import('./image-viewer.js')).createImageEditor(file, () => imageView(file, url));
    switch (file.kind) {
      case 'image':
        return imageView(file, url);
      case 'video':
        return (await import('./video-editor.js')).createVideoEditor(file, url);
      case 'audio':
        return h('div', { class: 'view-notice' }, h('h2', {}, file.name), h('audio', { src: url, controls: true, preload: 'metadata', 'aria-label': file.name }));
      default:
        return notice(file, t("Jay-E doesn't open this kind of file."));
    }
  } catch (error) {
    return notice(file, t("Couldn't open it: {error}", { error: error?.message ?? String(error) }));
  }
}
