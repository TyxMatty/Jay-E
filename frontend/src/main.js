import { initTheme } from './ui/theme.js';
import { mountApp } from './ui/app-shell.js';
import { settings } from './store/settings.js';
import { setLanguage, resolveLanguage } from './core/i18n.js';

setLanguage(resolveLanguage(settings.get().language));
initTheme();
mountApp(document.getElementById('app'));

// No browser menu (Back, Reload, Print, Inspect…) outside text fields. Dev builds keep it, for Inspect.
if (!import.meta.env.DEV) {
  document.addEventListener('contextmenu', (e) => {
    if (!e.target.closest?.('input, textarea, [contenteditable="true"]')) e.preventDefault();
  });
}
