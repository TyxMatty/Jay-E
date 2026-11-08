// Whether the interface animates: Settings › Appearance › Animations, On (default) or Off, as in
// Jay-C (core/motion.js). It doesn't follow Windows' "reduce motion", the line's choice. ui/theme.js
// puts the result on <html data-motion="full|reduced">, which the CSS animations key off.

export const MOTION_PREFERENCES = ['on', 'off'];

/** Only 'off' turns animations off; anything else is on. */
export const normalizeMotion = (preference) => (preference === 'off' ? 'off' : 'on');

/** @returns {'full' | 'reduced'} */
export const resolveMotion = (preference) => (normalizeMotion(preference) === 'off' ? 'reduced' : 'full');
