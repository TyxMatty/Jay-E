// The "Welcome!" tour shown on first launch (ui/welcome-modal.js) and from Settings › About,
// as in Jay-C. Content and navigation live here so they can be tested without a DOM.

/**
 * One card per step. `art` names the illustration (ui/welcome-art.js); `action` is an optional
 * button that opens a related screen.
 * @type {ReadonlyArray<{ key: string, art: string, title: string, body: string, action?: { label: string, target: 'settings' } }>}
 */
export const WELCOME_STEPS = Object.freeze([
  {
    key: 'welcome',
    art: 'welcome',
    title: 'Welcome to Jay-E!',
    body: "Read, edit and convert documents, images and videos, light on your PC, with no account and no office suite. Here's how it works, in a few quick steps.",
  },
  {
    key: 'open',
    art: 'open',
    title: 'Open your files',
    body: 'Click Open, drop files on the window or use "Open with" in Windows. Each file gets its own tab, and Jay-E knows its type by what is inside, not by the name.',
  },
  {
    key: 'pdf',
    art: 'pdf',
    title: 'PDFs',
    body: 'Read and edit PDFs: highlight, draw, add notes and text, fill in forms, and reorder, turn, delete or insert pages. Even a thousand pages scroll smoothly.',
  },
  {
    key: 'write',
    art: 'write',
    title: 'Markdown and LaTeX',
    body: 'Write with the preview beside the text, math included. LaTeX compiles inside Jay-E: the first time it downloads the packages your document uses, then it takes a second or two. Click an error to go to its line.',
  },
  {
    key: 'documents',
    art: 'documents',
    title: 'Word, OpenDocument and RTF',
    body: 'Edit Word and OpenDocument files on their own pages and fonts, with the usual toolbar: styles, bold, colors, lists, tables, pictures. Only what you change is rewritten. RTF opens to read and export.',
  },
  {
    key: 'media',
    art: 'media',
    title: 'Photos and videos',
    body: 'Crop, rotate and adjust photos, and convert many at once. Cut and join videos without re-encoding, in seconds, and convert them to MP4 or M4A.',
  },
  {
    key: 'yours',
    art: 'yours',
    title: 'Make it yours',
    body: 'Pick the Calm, Neon or Soft style, light or dark, and the language in Settings. Jay-E checks for a new version when it opens and updates itself.',
    action: { label: 'Open Settings', target: 'settings' },
  },
]);

/** Keeps a step index inside the tour. */
export function clampStep(index, count = WELCOME_STEPS.length) {
  if (!Number.isFinite(index)) return 0;
  return Math.min(Math.max(Math.trunc(index), 0), count - 1);
}

/** The tour opens by itself once (ui/app-shell.js marks it seen as it opens). */
export function shouldShowWelcome(prefs) {
  return prefs?.welcomeSeen !== true;
}
