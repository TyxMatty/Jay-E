// UI preferences persisted in localStorage. public/theme-init.js reads the same key.
const STORAGE_KEY = 'jaye.settings';

export const DEFAULT_SETTINGS = Object.freeze({
  /** 'system' | 'light' | 'dark' */
  theme: 'system',
  /** The look: 'calm', 'neon' or 'soft' (core/style.js, styles/theme.css). */
  style: 'calm',
  /** Interface language: 'system' (the OS language), 'en' or 'pt-BR' (core/i18n.js). */
  language: 'system',
  /** The Markdown editor's view: 'code', 'split' or 'preview' (ui/text-editor.js). */
  markdownMode: 'split',
  /** The image "Save as / convert" options last used (ui/image-convert.js). */
  imageExport: { format: 'webp', quality: 80, lossless: false, maxSide: null },
  /** The batch converter's format per kind of file, by group key (core/convert.js GROUPS). */
  convertTo: {},
  /** The "Welcome!" tour (core/welcome.js) already opened by itself once; Settings › About reopens it. */
  welcomeSeen: false,
});

function load() {
  try {
    return { ...DEFAULT_SETTINGS, ...JSON.parse(localStorage.getItem(STORAGE_KEY) ?? '{}') };
  } catch {
    return { ...DEFAULT_SETTINGS };
  }
}

let state = load();
const listeners = new Set();

export const settings = {
  get() {
    return state;
  },

  update(patch) {
    const prev = state;
    state = { ...state, ...patch };
    try {
      localStorage.setItem(STORAGE_KEY, JSON.stringify(state));
    } catch {
      // Storage unavailable (private mode / quota): keep working in memory.
    }
    for (const listener of listeners) listener(state, prev);
  },

  /** Calls `listener(next, prev)` on every update. Returns an unsubscribe function. */
  subscribe(listener) {
    listeners.add(listener);
    return () => listeners.delete(listener);
  },
};
