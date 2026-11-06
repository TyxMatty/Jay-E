// The app's look (Settings › Appearance › Style): <html data-style> picks a set of tokens in
// styles/theme.css (the same three styles as Jay-C). public/theme-init.js repeats resolveStyle().

export const STYLES = ['calm', 'neon', 'soft'];

/** A saved preference, or 'calm' when missing or unknown. */
export const resolveStyle = (preference) => (STYLES.includes(preference) ? preference : 'calm');
