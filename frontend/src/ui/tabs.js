// Accessible tabs (WAI-ARIA tabs pattern): one tab stop, arrow keys move between tabs (left/right,
// or up/down for a vertical list), Home/End jump, hidden tabs are skipped. Used by the setup guide
// and Settings.
import { h } from './dom.js';

/**
 * @param {{ idPrefix: string, label: string, className: string, vertical?: boolean,
 *   items: Array<{ key: string, label: string | Node, panel: HTMLElement }>, onSelect?: (key: string) => void }} options
 */
export function createTabs({ idPrefix, label, className, vertical = false, items, onSelect }) {
  const tabs = items.map(({ key, label: text, panel }) => {
    const tab = h('button', { type: 'button', role: 'tab', id: `${idPrefix}-tab-${key}`, 'aria-controls': `${idPrefix}-panel-${key}` }, text);
    panel.id = `${idPrefix}-panel-${key}`;
    panel.setAttribute('role', 'tabpanel');
    panel.setAttribute('aria-labelledby', tab.id);
    return { key, tab, panel };
  });
  const list = h('div', { class: className, role: 'tablist', 'aria-label': label, 'aria-orientation': vertical ? 'vertical' : 'horizontal' }, tabs.map((t) => t.tab));
  let current = null;

  const select = (key, { focus = false } = {}) => {
    current = key;
    for (const t of tabs) {
      const on = t.key === key;
      t.tab.setAttribute('aria-selected', String(on));
      t.tab.tabIndex = on ? 0 : -1;
      t.panel.hidden = !on;
      if (on && focus) t.tab.focus();
    }
    onSelect?.(key);
  };

  list.addEventListener('click', (e) => {
    const t = tabs.find((x) => x.tab === e.target.closest('[role="tab"]'));
    if (t) select(t.key);
  });
  list.addEventListener('keydown', (e) => {
    const shown = tabs.filter((t) => !t.tab.hidden);
    const index = shown.findIndex((t) => t.tab === document.activeElement);
    // A vertical list also takes left/right: on narrow screens it's laid out as a row.
    const steps = { ArrowRight: 1, ArrowLeft: -1, ...(vertical ? { ArrowDown: 1, ArrowUp: -1 } : {}) };
    const next = e.key === 'Home' ? 0 : e.key === 'End' ? shown.length - 1 : steps[e.key] !== undefined ? index + steps[e.key] : undefined;
    if (next === undefined || index < 0) return;
    e.preventDefault();
    select(shown[(next + shown.length) % shown.length].key, { focus: true });
  });

  return {
    list,
    select,
    get current() {
      return current;
    },
    /** Hides a tab (and its panel) that doesn't apply here, e.g. a feature this platform lacks. */
    setHidden(key, hidden) {
      const t = tabs.find((x) => x.key === key);
      if (!t) return;
      t.tab.hidden = hidden;
      if (hidden && current === key) select(tabs.find((x) => !x.tab.hidden).key);
    },
  };
}
