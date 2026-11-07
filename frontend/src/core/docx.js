// What the DOCX editor (ui/docx-editor.js) works out without the DOM: list markers, the look of a
// piece of text compared with its style, colors and font names from computed CSS.

/** The CSS class of a Word style id, as jay_docs::doc::edit writes it. */
export const styleClass = (id) => (id ? id.replace(/[^A-Za-z0-9-]/g, '_') : '_default');

/** The levels of a list the toolbar creates (jay_docs::doc::edit::default_level). */
export function defaultLevels(bullet) {
  return Array.from({ length: 9 }, (_, i) =>
    bullet
      ? { fmt: 'bullet', text: ['•', '◦', '▪'][i % 3], start: 1, left: 720 * (i + 1), hanging: 360 }
      : { fmt: ['decimal', 'lowerLetter', 'lowerRoman'][i % 3], text: `%${i + 1}.`, start: 1, left: 720 * (i + 1), hanging: 360 },
  );
}

const ROMAN = [
  [1000, 'm'],
  [900, 'cm'],
  [500, 'd'],
  [400, 'cd'],
  [100, 'c'],
  [90, 'xc'],
  [50, 'l'],
  [40, 'xl'],
  [10, 'x'],
  [9, 'ix'],
  [5, 'v'],
  [4, 'iv'],
  [1, 'i'],
];

/** A list number in a Word number format. */
export function formatNumber(n, fmt) {
  switch (fmt) {
    case 'lowerLetter':
    case 'upperLetter': {
      // Word repeats the letter after z: a … z, aa … zz.
      const letter = String.fromCharCode(97 + ((n - 1) % 26)).repeat(Math.floor((n - 1) / 26) + 1);
      return fmt === 'upperLetter' ? letter.toUpperCase() : letter;
    }
    case 'lowerRoman':
    case 'upperRoman': {
      let rest = Math.min(Math.max(n, 1), 3999);
      let out = '';
      for (const [value, digits] of ROMAN) {
        while (rest >= value) {
          out += digits;
          rest -= value;
        }
      }
      return fmt === 'upperRoman' ? out.toUpperCase() : out;
    }
    case 'decimalZero':
      return String(n).padStart(2, '0');
    default:
      return String(n);
  }
}

/**
 * The marker of each paragraph, in order: `nums[i]` is its "numId:level" (or "") and
 * `levels(numId)` the list's levels. Counting goes on through the document, as in Word.
 */
export function listMarkers(nums, levels) {
  const counters = new Map();
  return nums.map((num) => {
    const [id, lvlText] = (num || '').split(':');
    if (!id) return '';
    const defs = levels(id);
    const lvl = Math.min(Math.max(parseInt(lvlText, 10) || 0, 0), defs.length - 1);
    let count = counters.get(id);
    if (!count) counters.set(id, (count = defs.map((d) => d.start - 1)));
    count[lvl] += 1;
    for (let i = lvl + 1; i < count.length; i++) count[i] = defs[i].start - 1;
    const def = defs[lvl];
    if (def.fmt === 'bullet') return def.text;
    if (def.fmt === 'none') return '';
    return def.text.replace(/%([1-9])/g, (_, k) => {
      const i = Number(k) - 1;
      return defs[i] ? formatNumber(Math.max(count[i], defs[i].start), defs[i].fmt) : '';
    });
  });
}

/** `rgb(…)`/`rgba(…)` as 6 hex digits, or '' when transparent. */
export function toHex(css) {
  const m = /rgba?\(([^)]+)\)/.exec(css || '');
  if (!m) return '';
  const [r, g, b, a = 1] = m[1].split(/[\s,/]+/).filter(Boolean).map(Number);
  if (a === 0) return '';
  return [r, g, b].map((v) => Math.round(v).toString(16).padStart(2, '0')).join('');
}

/** The first family of a computed `font-family`, unquoted. */
export const firstFamily = (css) => (css || '').split(',')[0].trim().replace(/^["']|["']$/g, '');

/** A style's run properties (jay_docs RProps) as the editor compares them. */
export function normalProps(r = {}, over = null) {
  const out = {
    font: r.font ?? null,
    size: r.size ?? null,
    bold: !!r.bold,
    italic: !!r.italic,
    underline: !!r.underline,
    strike: !!r.strike,
    color: !r.color || r.color === 'auto' ? '000000' : r.color,
    bg: r.bg || '',
    vert: r.vert === 'superscript' || r.vert === 'subscript' ? r.vert : '',
  };
  // A character style changes only what it says.
  if (over) for (const [k, v] of Object.entries(normalProps(over))) if (over[k] != null) out[k] = v;
  return out;
}

const sameSize = (a, b) => a != null && b != null && Math.abs(a - b) <= 0.25;

/** Whether two looks are the same (sizes within a quarter point). */
export function sameProps(a, b) {
  if (!a || !b) return false;
  return (
    (a.font ?? '').toLowerCase() === (b.font ?? '').toLowerCase() &&
    sameSize(a.size, b.size) &&
    a.bold === b.bold &&
    a.italic === b.italic &&
    a.underline === b.underline &&
    a.strike === b.strike &&
    a.color === b.color &&
    a.bg === b.bg &&
    a.vert === b.vert
  );
}

/**
 * What to write on a run so it looks like `cur` over its styles' `base` (jay_docs edit::Set):
 * a value where they differ, null where the style already gives it.
 */
export function runSet(cur, base) {
  return {
    font: cur.font && cur.font.toLowerCase() !== (base.font ?? '').toLowerCase() ? cur.font : null,
    size: cur.size != null && !sameSize(cur.size, base.size) ? cur.size : null,
    bold: cur.bold !== base.bold ? cur.bold : null,
    italic: cur.italic !== base.italic ? cur.italic : null,
    underline: cur.underline !== base.underline ? cur.underline : null,
    strike: cur.strike !== base.strike ? cur.strike : null,
    color: cur.color !== base.color ? cur.color : null,
    bg: cur.bg !== base.bg ? cur.bg : null,
    vert: cur.vert || null,
  };
}
