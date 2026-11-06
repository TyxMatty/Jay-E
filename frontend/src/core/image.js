// The bridge to src-tauri/src/image.rs. Errors come back as { kind: 'toobig' | 'unsupported' |
// 'invalid' | 'io', width?, height?, message? }.

const core = () => import('@tauri-apps/api/core');
const call = async (cmd, args) => (await core()).invoke(cmd, args);

/** { id, width, height } (upright, EXIF orientation applied). */
export const openImage = (path) => call('image_open', { path });
export const closeImage = (id) => call('image_close', { id });
/** edits: { ops: [{ op: 'rotateRight' | 'rotateLeft' | 'flipH' | 'flipV' | 'crop', x, y, w, h }], adjust: { brightness, contrast, saturation } } → { rev, width, height } */
export const setEdits = (id, edits) => call('image_set_edits', { id, edits });
/** options: { format, quality, lossless, maxSide } → the saved path, or null if cancelled. */
export const saveImageAs = (id, options) => call('image_save_as', { id, options });
export const cancelConvert = () => call('image_convert_cancel');

/** Converts into a folder the person picks; events: started, done, failed, finished. Resolves to false if cancelled at the folder. */
export async function convertImages(items, options, onEvent) {
  const { Channel } = await core();
  const channel = new Channel();
  channel.onmessage = onEvent;
  return call('image_convert', { items, options, channel });
}

let base;
/** URLs of the jaye:// protocol for an image: the whole view, or a region of the full-size image. */
export async function imageUrls() {
  base ??= (await core()).convertFileSrc('', 'jaye');
  return {
    view: (id, rev, width) => `${base}img/${id}/${rev}/view/${Math.max(1, Math.round(width))}`,
    region: (id, rev, x, y, w, h, width) => `${base}img/${id}/${rev}/region/${[x, y, w, h, width].map((n) => Math.max(0, Math.round(n))).join('/')}`,
  };
}

/** Output formats, in the order the menu shows them. */
export const FORMATS = [
  ['webp', 'WebP'],
  ['avif', 'AVIF'],
  ['jpeg', 'JPEG'],
  ['png', 'PNG'],
  ['gif', 'GIF'],
  ['bmp', 'BMP'],
  ['tiff', 'TIFF'],
  ['ico', 'ICO'],
  ['qoi', 'QOI'],
];
export const LOSSY = new Set(['jpeg', 'webp', 'avif']);
