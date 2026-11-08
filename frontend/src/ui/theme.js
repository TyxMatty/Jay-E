import { settings } from '../store/settings.js';
import { resolveStyle } from '../core/style.js';
import { resolveMotion } from '../core/motion.js';
import { updateWindowIcon } from './window-icon.js';

const darkQuery = matchMedia('(prefers-color-scheme: dark)');
const isTauri = '__TAURI_INTERNALS__' in window;

/** Same logic as public/theme-init.js. */
export function resolveTheme(preference) {
  return preference === 'dark' || (preference === 'system' && darkQuery.matches) ? 'dark' : 'light';
}

function applyTheme() {
  const theme = resolveTheme(settings.get().theme);
  const root = document.documentElement;
  if (root.dataset.theme === theme) return;
  root.dataset.theme = theme;

  // Keep the native title bar in sync.
  if (isTauri) {
    import('@tauri-apps/api/window')
      .then(({ getCurrentWindow }) => getCurrentWindow().setTheme(theme))
      .catch(() => {});
  }
}

const applyStyle = () => (document.documentElement.dataset.style = resolveStyle(settings.get().style));
/** <html data-motion="full|reduced"> from Settings › Appearance › Animations. */
const applyMotion = () => (document.documentElement.dataset.motion = resolveMotion(settings.get().motion));

export function initTheme() {
  document.documentElement.dataset.theme = ''; // force the first applyTheme() to sync the title bar
  applyTheme();
  applyStyle();
  applyMotion();
  updateWindowIcon();
  settings.subscribe((next, prev) => {
    if (next.theme !== prev.theme) applyTheme();
    if (next.motion !== prev.motion) applyMotion();
    if (next.style !== prev.style) {
      applyStyle();
      // The window and taskbar icon follow the style.
      updateWindowIcon();
    }
  });
  darkQuery.addEventListener('change', applyTheme);
}

/** Header quick toggle: flips between explicit light and dark. */
export function toggleTheme() {
  settings.update({ theme: currentTheme() === 'dark' ? 'light' : 'dark' });
}

export const currentTheme = () => document.documentElement.dataset.theme;
