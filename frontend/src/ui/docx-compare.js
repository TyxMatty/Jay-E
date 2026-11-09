// "Compare with…" in the document editor (stage 10): the saved file against an older version the
// person picks (jay_docs::doc::compare), paragraph by paragraph and word by word, in a dialog.
import { h } from './dom.js';
import { t } from '../core/i18n.js';

const TAG = { added: 'ins', removed: 'del', same: 'span' };

/** The comparison as elements: one per paragraph, its changed words marked. */
export function comparisonView(paragraphs) {
  return paragraphs.map((p) => h('p', { class: `dx-cmp-p is-${p.kind}` }, p.words.map((w) => h(TAG[w.kind] ?? 'span', { class: `dx-cmp-${w.kind}` }, w.text))));
}

/** Shows `compared` ({ other, paragraphs }) for the file `name`. */
export function showComparison(compared, name) {
  const counts = { added: 0, removed: 0, changed: 0 };
  for (const p of compared.paragraphs) if (p.kind in counts) counts[p.kind]++;
  const body = h('div', { class: 'dx-cmp-body', tabindex: '0' }, comparisonView(compared.paragraphs));
  const onlyBox = h('input', { type: 'checkbox', id: 'dx-cmp-only' });
  onlyBox.addEventListener('change', () => body.classList.toggle('only-changes', onlyBox.checked));
  const same = !counts.added && !counts.removed && !counts.changed;
  const dialog = h(
    'dialog',
    { class: 'img-dialog dx-cmp-dialog', 'aria-labelledby': 'dx-cmp-title' },
    h('h2', { id: 'dx-cmp-title' }, t('{old} → {new}', { old: compared.other, new: name })),
    h(
      'p',
      { class: 'hint' },
      same ? t('The text is the same in both.') : t('{changed} changed, {added} added and {removed} removed paragraphs. Only the text is compared, not the formatting.', counts),
      ' ',
      h('ins', { class: 'dx-cmp-added' }, t('added')),
      ' ',
      h('del', { class: 'dx-cmp-removed' }, t('removed')),
    ),
    h('label', { class: 'dx-cmp-only', for: 'dx-cmp-only' }, onlyBox, t('Only the changes')),
    body,
    h('div', { class: 'img-dialog-actions' }, h('button', { type: 'button', class: 'btn', onClick: () => dialog.close() }, t('Close'))),
  );
  dialog.addEventListener('close', () => dialog.remove());
  document.body.append(dialog);
  dialog.showModal();
}
