// Runs before CSS paints. Keep in sync with resolveTheme() in src/ui/theme.js, resolveStyle() in
// src/core/style.js and resolveMotion() in src/core/motion.js.
(function () {
  var theme = 'light';
  var style = 'calm';
  var motion = 'full';
  try {
    var saved = JSON.parse(localStorage.getItem('jaye.settings') || '{}');
    var pref = saved.theme || 'system';
    var dark = pref === 'dark' || (pref === 'system' && matchMedia('(prefers-color-scheme: dark)').matches);
    theme = dark ? 'dark' : 'light';
    if (saved.style === 'neon' || saved.style === 'soft') style = saved.style;
    if (saved.motion === 'off') motion = 'reduced';
  } catch (e) {}
  document.documentElement.dataset.theme = theme;
  document.documentElement.dataset.style = style;
  document.documentElement.dataset.motion = motion;
})();
