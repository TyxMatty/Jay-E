// The bridge to src-tauri/src/pdf.rs. Pages arrive as PNGs from the jaye:// protocol.

const core = () => import('@tauri-apps/api/core');
const call = async (cmd, args) => (await core()).invoke(cmd, args);

/** { doc, pages: [[w, h] in points] }; rejects with 'password', 'invalid', 'toobig' or { io }. */
export const openPdf = (path, password) => call('pdf_open', { path, password: password ?? null });
export const closePdf = (doc) => call('pdf_close', { doc });
export const setVisible = (doc, first, last) => call('pdf_visible', { doc, first, last });
/** [{ text, x, y, w, h, xs }] in points from the page's top-left. */
export const pageText = (doc, page) => call('pdf_text', { doc, page });
/** [{ rect: [x, y, w, h], target: { kind: 'page', page, top } | { kind: 'uri', uri } }] */
export const pageLinks = (doc, page) => call('pdf_links', { doc, page });
export const openLink = (uri) => call('open_link', { uri });
export const cancelSearch = (doc) => call('pdf_cancel_search', { doc });

/** Streams { event: 'hits', page, rects } | { event: 'progress', searched } | { event: 'done', searched }. */
export async function search(doc, query, onEvent) {
  const { Channel } = await core();
  const channel = new Channel();
  channel.onmessage = onEvent;
  return call('pdf_search', { doc, query, channel });
}

let base;
/** The page drawn `width` pixels wide, decoded off the main thread; null if skipped. Close it after drawing. */
export async function fetchPage(doc, page, width, { thumb = false, signal } = {}) {
  // http://jaye.localhost/ on Windows, jaye://localhost/ elsewhere.
  base ??= (await core()).convertFileSrc('', 'jaye');
  const response = await fetch(`${base}pdf/${doc}/${page}/${Math.round(width)}${thumb ? '?thumb' : ''}`, { signal });
  if (response.status !== 200) return null;
  return createImageBitmap(await response.blob());
}
