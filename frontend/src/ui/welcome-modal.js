import { h } from './dom.js';
import { WELCOME_ART } from './welcome-art.js';
import { WELCOME_STEPS, clampStep } from '../core/welcome.js';
import { t } from '../core/i18n.js';

const reducedMotion = () => matchMedia('(prefers-reduced-motion: reduce)').matches;

/**
 * The "Welcome!" tour, as in Jay-C: one card per step (core/welcome.js), Back/Next, dots, Skip.
 * Skip, Get started and Esc close it.
 * @param {{ openSettings: () => void }} actions
 */
export function createWelcomeModal({ openSettings }) {
  let index = 0;

  const art = h('div', { class: 'welcome-art', 'aria-hidden': 'true' });
  const counter = h('p', { class: 'welcome-counter' });
  const title = h('h2', { id: 'welcome-title', class: 'welcome-title' });
  const body = h('p', { class: 'welcome-text' });
  const action = h('button', { type: 'button', class: 'btn btn-secondary welcome-action' });
  // Announces the new step to screen readers when Back/Next is used.
  const content = h('div', { class: 'welcome-content', 'aria-live': 'polite' }, counter, title, body);

  const dotButtons = WELCOME_STEPS.map((step, i) =>
    h('button', { type: 'button', class: 'welcome-dot', 'aria-label': t('Step {n}: {title}', { n: i + 1, title: t(step.title) }), onClick: () => go(i) }),
  );
  const dots = h('div', { class: 'welcome-dots' }, dotButtons);

  const skipButton = h('button', { type: 'button', class: 'btn btn-secondary', onClick: () => dialog.close() }, t('Skip'));
  const backButton = h('button', { type: 'button', class: 'btn btn-secondary', onClick: () => go(index - 1) }, t('Back'));
  const nextButton = h('button', { type: 'button', class: 'btn', onClick: next }, t('Next'));

  const dialog = h(
    'dialog',
    { class: 'welcome-modal', 'aria-labelledby': 'welcome-title' },
    h('div', { class: 'welcome-body' }, art, content, action, dots),
    h('footer', { class: 'welcome-footer' }, skipButton, h('span', { class: 'welcome-spacer' }), backButton, nextButton),
  );

  function next() {
    if (index === WELCOME_STEPS.length - 1) dialog.close();
    else go(index + 1);
  }

  function go(target) {
    const from = index;
    index = clampStep(target);
    const step = WELCOME_STEPS[index];
    const last = index === WELCOME_STEPS.length - 1;
    dialog.dataset.step = step.key;
    art.innerHTML = WELCOME_ART[step.art] ?? '';
    counter.textContent = t('Step {n} of {total}', { n: index + 1, total: WELCOME_STEPS.length });
    title.textContent = t(step.title);
    body.textContent = t(step.body);
    // Hidden but still taking its room, so the card doesn't change height between steps.
    action.style.visibility = step.action ? '' : 'hidden';
    action.textContent = step.action ? t(step.action.label) : '';
    action.disabled = !step.action;
    dotButtons.forEach((dot, i) => (i === index ? dot.setAttribute('aria-current', 'step') : dot.removeAttribute('aria-current')));
    backButton.disabled = index === 0;
    nextButton.textContent = last ? t('Get started') : t('Next');
    skipButton.hidden = last;
    // The new card slides in from the side it comes from.
    if (from !== index && !reducedMotion()) {
      const dx = index > from ? 16 : -16;
      for (const el of [art, content]) el.animate([{ opacity: 0, translate: `${dx}px 0` }, { opacity: 1, translate: '0 0' }], { duration: 220, easing: 'ease-out' });
    }
  }

  /** The text area as tall as the longest step, so the card keeps one height (and the buttons stay put). */
  function fitHeight() {
    content.style.minHeight = '';
    let tallest = 0;
    for (const step of WELCOME_STEPS) {
      title.textContent = t(step.title);
      body.textContent = t(step.body);
      tallest = Math.max(tallest, content.offsetHeight);
    }
    content.style.minHeight = `${tallest}px`;
    go(index);
  }

  action.addEventListener('click', () => {
    if (WELCOME_STEPS[index].action?.target === 'settings') {
      dialog.close();
      openSettings();
    }
  });

  // Left/right arrows move between steps.
  dialog.addEventListener('keydown', (e) => {
    if (e.altKey || e.ctrlKey || e.metaKey || e.shiftKey) return;
    if (e.key === 'ArrowRight') go(index + 1);
    else if (e.key === 'ArrowLeft') go(index - 1);
    else return;
    e.preventDefault();
  });

  go(0);
  addEventListener('resize', () => dialog.open && fitHeight());

  return {
    element: dialog,
    open() {
      index = 0;
      dialog.showModal();
      fitHeight(); // measured once laid out: depends on the width and the language
      nextButton.focus();
    },
  };
}
