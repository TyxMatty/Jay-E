// The app's look (Settings › Appearance › Style): <html data-style> picks a set of tokens in
// styles/theme.css (the same three styles as Jay-C). public/theme-init.js repeats resolveStyle().

export const STYLES = ['calm', 'neon', 'soft'];

/** A saved preference, or 'calm' when missing or unknown. */
export const resolveStyle = (preference) => (STYLES.includes(preference) ? preference : 'calm');

/**
 * The window and taskbar icon's colors per style (ui/window-icon.js): the lines and the inside of
 * the wolf, the dark-theme ones, so the icon reads on light and dark taskbars. Same as Jay-C's.
 */
export const ICON_COLORS = {
  calm: { lines: '#aa8bff', fill: '#0f0d16' },
  neon: { lines: '#22d3ee', fill: '#06060a' },
  soft: { lines: '#ff9068', fill: '#16131b' },
};
