// Illustrations for the "Welcome!" tour (ui/welcome-modal.js), one small scene per step, drawn as
// in Jay-C. Trusted static SVG (like icons.js). Colors come from the theme: `currentColor` is the
// accent, the .wa-* classes map to theme variables. Motion lives in app.css (.wa-* keyframes) and
// only runs without prefers-reduced-motion; otherwise each scene is a still picture.

const svg = (body) =>
  `<svg viewBox="0 0 240 140" fill="none" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${body}</svg>`;

/** A page with a folded corner and `lines` text lines; `cls`/`style` add animation. */
const page = (x, y, w, hgt, lines, cls = '', style = '') =>
  `<g class="${cls}" style="${style}"><path d="M${x} ${y} h${w - 12} l12 12 v${hgt - 12} h-${w} z" class="wa-card"/>` +
  `<path d="M${x + w - 12} ${y} v12 h12" class="wa-card"/>` +
  Array.from({ length: lines }, (_, i) => `<path d="M${x + 9} ${y + 20 + i * 10} h${i % 3 === 2 ? w * 0.4 : w - 22}" class="wa-stroke-muted wa-thin"/>`).join('') +
  `</g>`;

/** A photo: a frame with a sun and a mountain. */
const photo = (x, y, w, hgt, cls = '', style = '') =>
  `<g class="${cls}" style="${style}"><rect x="${x}" y="${y}" width="${w}" height="${hgt}" rx="6" class="wa-card"/>` +
  `<circle cx="${x + w * 0.72}" cy="${y + hgt * 0.32}" r="${hgt * 0.12}" class="wa-fill-accent"/>` +
  `<path d="M${x + 6} ${y + hgt - 6} l${w * 0.3} ${-hgt * 0.45} l${w * 0.2} ${hgt * 0.25} l${w * 0.12} ${-hgt * 0.12} l${w * 0.3} ${hgt * 0.32} z" class="wa-fill-accent wa-dim"/></g>`;

/** A video: a frame with a play button. */
const video = (x, y, w, hgt, cls = '', style = '') =>
  `<g class="${cls}" style="${style}"><rect x="${x}" y="${y}" width="${w}" height="${hgt}" rx="6" class="wa-card"/>` +
  `<circle cx="${x + w / 2}" cy="${y + hgt / 2}" r="${hgt * 0.24}" class="wa-fill-accent"/>` +
  `<path d="M${x + w / 2 - 3} ${y + hgt / 2 - 6} l9 6 l-9 6 z" class="wa-fill-surface"/></g>`;

export const WELCOME_ART = {
  // A document, a photo and a video arriving together.
  welcome: svg(
    page(30, 26, 56, 76, 5, 'wa-pop', '--d:0ms') +
      photo(96, 40, 62, 50, 'wa-pop', '--d:150ms') +
      video(166, 40, 50, 50, 'wa-pop', '--d:300ms') +
      `<path d="M40 120 h160" class="wa-stroke-accent wa-thin wa-dim"/>`,
  ),
  // A window with tabs; a file falls into it and a new tab appears.
  open: svg(
    `<rect x="40" y="38" width="160" height="92" rx="10" class="wa-card"/>` +
      `<path d="M40 60 h160" class="wa-stroke-muted wa-thin"/>` +
      `<rect x="50" y="44" width="34" height="12" rx="4" class="wa-fill-muted"/>` +
      `<rect x="90" y="44" width="34" height="12" rx="4" class="wa-fill-muted"/>` +
      `<rect x="130" y="44" width="34" height="12" rx="4" class="wa-fill-accent wa-pop" style="--d:700ms"/>` +
      `<rect x="72" y="74" width="96" height="44" rx="8" class="wa-dash wa-flow"/>` +
      page(104, 8, 32, 42, 2, 'wa-drop'),
  ),
  // A viewer window with pages scrolling past, and a magnifier over the one in view.
  pdf: svg(
    `<clipPath id="wa-pdf-view"><rect x="78" y="12" width="96" height="116" rx="8"/></clipPath>` +
      `<rect x="78" y="12" width="96" height="116" rx="8" class="wa-fill-muted"/>` +
      `<g clip-path="url(#wa-pdf-view)"><g class="wa-scroll">` +
      page(94, -38, 60, 80, 5) +
      page(94, 52, 60, 80, 5) +
      page(94, 142, 60, 80, 5) +
      `</g></g>` +
      `<rect x="162" y="20" width="6" height="100" rx="3" class="wa-fill-surface"/>` +
      `<rect x="162" y="22" width="6" height="24" rx="3" class="wa-fill-accent wa-thumb"/>` +
      `<g class="wa-float"><circle cx="70" cy="66" r="16" class="wa-glass"/><path d="M82 78 l12 12" class="wa-stroke-text"/></g>`,
  ),
  // The text on the left, its preview (a title and a formula) on the right, the caret blinking.
  write: svg(
    `<rect x="20" y="20" width="96" height="100" rx="8" class="wa-card"/>` +
      `<path d="M32 38 h24 M32 52 h60 M32 64 h48 M32 80 h40 M32 92 h56" class="wa-stroke-muted wa-thin"/>` +
      `<path d="M92 92 v-10" class="wa-stroke-accent wa-caret"/>` +
      `<path d="M120 70 h8" class="wa-stroke-accent wa-flow"/>` +
      `<rect x="132" y="20" width="88" height="100" rx="8" class="wa-card"/>` +
      `<path d="M144 38 h40" class="wa-stroke-text"/>` +
      `<path d="M144 54 h64 M144 66 h52" class="wa-stroke-muted wa-thin"/>` +
      `<path d="M168 84 h-14 l8 9 l-8 9 h14" class="wa-stroke-accent wa-thin wa-pop" style="--d:300ms"/>` +
      `<path d="M176 93 h20 M186 86 v14" class="wa-stroke-accent wa-thin wa-pop" style="--d:450ms"/>`,
  ),
  // An A4 sheet sending copies out in other formats.
  documents: svg(
    page(84, 14, 72, 104, 8) +
      [
        [26, 30],
        [26, 84],
        [184, 30],
        [184, 84],
      ]
        .map(
          ([x, y], i) =>
            `<path d="M${x < 120 ? 84 : 156} ${y + 13} L${x < 120 ? x + 34 : x}  ${y + 13}" class="wa-link wa-flow"/>` +
            `<rect x="${x}" y="${y}" width="34" height="26" rx="6" class="wa-fill-accent wa-pop" style="--d:${i * 120}ms"/>` +
            `<path d="M${x + 8} ${y + 13} h18" class="wa-stroke-surface wa-thin"/>`,
        )
        .join(''),
  ),
  // A photo being cropped, and a video strip cut without re-encoding.
  media: svg(
    photo(20, 18, 92, 66) +
      `<rect x="34" y="28" width="62" height="46" class="wa-crop wa-breathe"/>` +
      `<rect x="20" y="98" width="200" height="28" rx="6" class="wa-card"/>` +
      Array.from({ length: 6 }, (_, i) => `<rect x="${28 + i * 32}" y="104" width="24" height="16" rx="3" class="wa-fill-accent ${i === 2 || i === 3 ? '' : 'wa-dim'}"/>`).join('') +
      `<path d="M88 92 v40 M152 92 v40" class="wa-cut"/>` +
      video(140, 18, 80, 66, 'wa-float'),
  ),
  // The three styles, a light/dark switch and an update arrow.
  yours: svg(
    ['#7c5cff', '#06b6d4', '#f97316']
      .map((c, i) => `<g class="wa-pop" style="--d:${i * 120}ms"><rect x="${14 + i * 50}" y="26" width="44" height="60" rx="10" class="wa-card"/><circle cx="${36 + i * 50}" cy="48" r="10" fill="${c}"/><path d="M${24 + i * 50} 70 h24" stroke="${c}" stroke-width="5"/></g>`)
      .join('') +
      `<rect x="30" y="102" width="56" height="24" rx="12" class="wa-fill-muted"/>` +
      `<circle cx="74" cy="114" r="9" class="wa-fill-accent wa-toggle"/>` +
      `<circle cx="200" cy="64" r="26" class="wa-card"/>` +
      `<g class="wa-spin"><path d="M188 58 a13 13 0 0 1 24 -2 M212 70 a13 13 0 0 1 -24 2" class="wa-stroke-accent wa-thin"/>` +
      `<path d="M212 48 v8 h-8 M188 80 v-8 h8" class="wa-stroke-accent wa-thin"/></g>` +
      `<path d="M120 108 h82 M120 120 h52" class="wa-stroke-muted wa-thin"/>`,
  ),
};
