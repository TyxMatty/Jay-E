// The window's icon (title bar and taskbar): the Jay wolf in the style's colors, drawn from two
// masks (assets/brand): the lines, and the inside of the head, as in Jay-C (ui/window-icon.js),
// which the Jay line's icons follow. The .exe's own icon (on disk, in the Start menu) is the file one.
import { ICON_COLORS, resolveStyle } from '../core/style.js';
import linesUrl from '../assets/brand/wolf-lines.png';
import fillUrl from '../assets/brand/wolf-fill.png';

const SIZE = 64;

const loadImage = (src) =>
  new Promise((resolve, reject) => Object.assign(new Image(), { onload: (e) => resolve(e.target), onerror: reject, src }));
let masks = null;

/** One mask painted in `color`, at `size` px. */
function tinted(img, color, size) {
  const c = Object.assign(document.createElement('canvas'), { width: size, height: size });
  const g = c.getContext('2d');
  g.imageSmoothingQuality = 'high';
  g.drawImage(img, 0, 0, size, size);
  g.globalCompositeOperation = 'source-in';
  g.fillStyle = color;
  g.fillRect(0, 0, size, size);
  return c;
}

let pending = null;

/** Redraws the window icon for the current style (desktop app only; a no-op in the browser). */
export function updateWindowIcon() {
  if (!('__TAURI_INTERNALS__' in window) || pending) return;
  // After the style attribute has been applied, once per frame.
  pending = requestAnimationFrame(async () => {
    pending = null;
    try {
      masks ??= Promise.all([loadImage(linesUrl), loadImage(fillUrl)]);
      const [lines, fill] = await masks;
      const colors = ICON_COLORS[resolveStyle(document.documentElement.dataset.style)];
      const canvas = Object.assign(document.createElement('canvas'), { width: SIZE, height: SIZE });
      const g = canvas.getContext('2d');
      g.drawImage(tinted(fill, colors.fill, SIZE), 0, 0);
      g.drawImage(tinted(lines, colors.lines, SIZE), 0, 0);
      const [{ Image }, { getCurrentWindow }, { invoke }] = await Promise.all([import('@tauri-apps/api/image'), import('@tauri-apps/api/window'), import('@tauri-apps/api/core')]);
      const pixels = g.getImageData(0, 0, SIZE, SIZE).data;
      // The buffer, not the Uint8ClampedArray: Tauri only treats Uint8Array/ArrayBuffer as bytes.
      await getCurrentWindow().setIcon(await Image.new(pixels.buffer, SIZE, SIZE)); // title bar
      // The taskbar's big icon: Tauri's setIcon doesn't set it (src-tauri/src/taskbar_icon.rs).
      await invoke('set_taskbar_icon', { rgba: Array.from(pixels), size: SIZE });
    } catch (error) {
      console.warn('[window-icon]', error);
    }
  });
}
