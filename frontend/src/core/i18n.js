// Interface language. Texts are written in English in the code and wrapped in t(); a language file
// (src/locales/*.js) maps each English text to its translation. Missing entries fall back to English.
// The language is fixed for the page's lifetime: changing it reloads the app (ui/app-shell.js).
import ptBR from '../locales/pt-BR.js';

/** Settings › Appearance › Language. 'system' follows the OS / WebView language. */
export const LANGUAGE_PREFERENCES = ['system', 'en', 'pt-BR'];

const TRANSLATIONS = { 'pt-BR': ptBR };

let current = 'en';

/** @returns {'en' | 'pt-BR'} */
export function resolveLanguage(preference, systemLanguage = globalThis.navigator?.language ?? 'en') {
  if (preference === 'en' || preference === 'pt-BR') return preference;
  return /^pt\b/i.test(systemLanguage) ? 'pt-BR' : 'en';
}

export function setLanguage(language) {
  current = TRANSLATIONS[language] ? language : 'en';
  if (globalThis.document) document.documentElement.lang = current;
}

export const language = () => current;

/**
 * The text in the current language. `{name}` placeholders are filled from `vars`.
 * @param {string} text the English text (also the lookup key)
 * @param {Record<string, string | number>} [vars]
 */
export function t(text, vars) {
  const translated = TRANSLATIONS[current]?.[text] ?? text;
  return vars ? translated.replace(/\{(\w+)\}/g, (match, key) => (key in vars ? String(vars[key]) : match)) : translated;
}
