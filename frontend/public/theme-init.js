// Runs before CSS paints. Keep in sync with resolveTheme() in src/ui/theme.js and resolveStyle() in
// src/core/style.js.
(function () {
  var theme = 'light';
  var style = 'calm';
  try {
    var saved = JSON.parse(localStorage.getItem('jaye.settings') || '{}');
    var pref = saved.theme || 'system';
    var dark = pref === 'dark' || (pref === 'system' && matchMedia('(prefers-color-scheme: dark)').matches);
    theme = dark ? 'dark' : 'light';
    if (saved.style === 'neon' || saved.style === 'soft') style = saved.style;
  } catch (e) {}
  document.documentElement.dataset.theme = theme;
  document.documentElement.dataset.style = style;
})();
