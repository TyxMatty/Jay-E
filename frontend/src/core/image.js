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
/** { r, g, b, l }: 256 counts each, of the image after crop/rotate/flip (image.rs `histogram`). */
export const imageHistogram = (id) => call('image_histogram', { id });
/** A `.cube` color look the person picks (checked in Rust): { path, name }, or null if cancelled. */
export const pickLut = () => call('image_pick_lut');
/** { bytes, exact, preview }: the file size in `options` (and the compressed look, for lossy formats). */
export const estimateImage = (id, options) => call('image_estimate', { id, options });

/** The tone and color settings that change nothing (jay_docs::image::Adjust). */
export const neutralAdjust = () => ({
  brightness: 0,
  contrast: 0,
  saturation: 0,
  exposure: 0,
  shadows: 0,
  highlights: 0,
  temperature: 0,
  tint: 0,
  vibrance: 0,
  grayscale: false,
  levels: { black: [0, 0, 0], white: [1, 1, 1], gamma: 1 },
  curve: [],
  lut: null,
  denoise: 0,
  sharpen: 0,
  blur: 0,
  vignette: 0,
});

/** The values with `clip` of the pixels below and above them: the darkest and lightest that count. */
function ends(counts, clip = 0.001) {
  const total = counts.reduce((a, b) => a + b, 0);
  if (!total) return [0, 255];
  const limit = total * clip;
  let lo = 0;
  for (let sum = 0; lo < 255 && (sum += counts[lo]) <= limit; lo++);
  let hi = 255;
  for (let sum = 0; hi > 0 && (sum += counts[hi]) <= limit; hi--);
  return hi > lo ? [lo, hi] : [0, 255];
}
const mean = (counts) => {
  const total = counts.reduce((a, b) => a + b, 0);
  return total ? counts.reduce((sum, n, v) => sum + n * v, 0) / total : 128;
};

/**
 * What the Auto buttons set, from the histogram: 'tone' stretches each channel to black and white
 * on its own; 'contrast' stretches all three by the same amount (colors kept); 'color' stretches
 * each channel and then makes the average gray with temperature and tint.
 */
export function autoAdjust(hist, mode) {
  const channels = [hist.r, hist.g, hist.b];
  let pairs;
  if (mode === 'contrast') {
    const all = hist.r.map((n, v) => n + hist.g[v] + hist.b[v]);
    pairs = channels.map(() => ends(all));
  } else pairs = channels.map((c) => ends(c));
  const levels = { black: pairs.map(([lo]) => lo / 255), white: pairs.map(([, hi]) => hi / 255), gamma: 1 };
  if (mode !== 'color') return { levels, temperature: 0, tint: 0 };
  // The channels' averages after the stretch, then the warmth and tint that make them equal.
  const [r, g, b] = channels.map((c, i) => Math.min(Math.max((mean(c) - pairs[i][0]) / (pairs[i][1] - pairs[i][0]), 0.01), 1));
  // Gentle: at most half the slider, so a picture that is meant to be colored isn't turned gray.
  const clamp = (v) => Math.round(Math.min(Math.max(v, -0.5), 0.5) * 100) / 100;
  return { levels, temperature: clamp((b - r) / (0.2 * (r + b))), tint: clamp((1 - (r + b) / 2 / g) / 0.15) };
}

/**
 * The tone curve at `x` (0..1): the same monotone cubic as image.rs `Curve`, for drawing it.
 * `points` are [in, out] pairs sorted by `in`; fewer than two means the straight line.
 */
export function curveAt(points, x) {
  const n = points.length;
  if (n < 2) return x;
  const xs = points.map((p) => p[0]);
  const ys = points.map((p) => p[1]);
  if (x <= xs[0]) return ys[0];
  if (x >= xs[n - 1]) return ys[n - 1];
  const d = xs.slice(0, -1).map((_, i) => (ys[i + 1] - ys[i]) / (xs[i + 1] - xs[i]));
  const m = xs.map((_, i) => (i === 0 ? d[0] : i === n - 1 ? d[n - 2] : d[i - 1] * d[i] <= 0 ? 0 : (d[i - 1] + d[i]) / 2));
  for (let i = 0; i < n - 1; i++) {
    if (d[i] === 0) {
      m[i] = m[i + 1] = 0;
      continue;
    }
    const a = m[i] / d[i];
    const b = m[i + 1] / d[i];
    const hyp = Math.hypot(a, b);
    if (hyp > 3) {
      m[i] = (3 / hyp) * a * d[i];
      m[i + 1] = (3 / hyp) * b * d[i];
    }
  }
  let i = 0;
  while (xs[i + 1] <= x) i++;
  const h = xs[i + 1] - xs[i];
  const t = (x - xs[i]) / h;
  const t2 = t * t;
  const t3 = t2 * t;
  return (2 * t3 - 3 * t2 + 1) * ys[i] + (t3 - 2 * t2 + t) * h * m[i] + (-2 * t3 + 3 * t2) * ys[i + 1] + (t3 - t2) * h * m[i + 1];
}

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
    original: (id, width) => `${base}img/${id}/0/original/${Math.max(1, Math.round(width))}`,
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
