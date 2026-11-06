// "Save as / Convert" for images: the format and its options, then either this image (with its
// edits) through a save dialog, or every open image into a folder, several at a time, with
// progress and a cancel button. Files are never overwritten: Rust picks "name (2).ext" instead.
import { h } from './dom.js';
import { t } from '../core/i18n.js';
import { settings } from '../store/settings.js';
import { saveImageAs, convertImages, cancelConvert, FORMATS, LOSSY } from '../core/image.js';
import { openImageFiles } from './files.js';

export function openConvertDialog({ file, id, getEdits, onSaved }) {
  const saved = settings.get().imageExport;
  const format = h('select', { class: 'input', id: 'img-format' }, FORMATS.map(([value, name]) => h('option', { value, selected: value === saved.format }, name)));
  const quality = h('input', { type: 'range', id: 'img-quality', min: '1', max: '100', value: String(saved.quality) });
  const qualityValue = h('output', { class: 'hint', for: 'img-quality' }, String(saved.quality));
  const lossless = h('input', { type: 'checkbox', id: 'img-lossless', checked: saved.lossless });
  const maxSide = h('input', { class: 'input', type: 'number', id: 'img-max', min: '1', max: '65535', placeholder: t('Original'), value: saved.maxSide ?? '' });
  const qualityRow = h('div', { class: 'field' }, h('label', { for: 'img-quality' }, t('Quality')), h('div', { class: 'img-slider' }, quality, qualityValue));
  const losslessRow = h('label', { class: 'field img-check' }, lossless, t('Without loss (bigger file)'));
  const status = h('p', { class: 'hint', role: 'status' });
  const progress = h('ol', { class: 'img-progress' });

  const options = () => ({
    format: format.value,
    quality: Number(quality.value),
    lossless: lossless.checked,
    maxSide: Number(maxSide.value) > 0 ? Math.round(Number(maxSide.value)) : null,
  });
  const syncOptions = () => {
    qualityValue.textContent = quality.value;
    qualityRow.hidden = !LOSSY.has(format.value) || (format.value === 'webp' && lossless.checked);
    losslessRow.hidden = format.value !== 'webp';
    settings.update({ imageExport: options() });
  };
  for (const el of [format, quality, lossless, maxSide]) el.addEventListener('input', syncOptions);

  const others = openImageFiles();
  let running = false;
  const saveButton = h('button', { type: 'button', class: 'btn' }, t('Save this image as…'));
  const convertButton = h('button', { type: 'button', class: 'btn btn-secondary' }, t('Convert all open images ({count})…', { count: others.length }));
  const cancelButton = h('button', { type: 'button', class: 'btn btn-danger', hidden: true, onClick: () => cancelConvert().catch(() => {}) }, t('Stop'));
  const closeButton = h('button', { type: 'button', class: 'btn btn-secondary', onClick: () => dialog.close() }, t('Close'));
  const setRunning = (on) => {
    running = on;
    saveButton.disabled = convertButton.disabled = on;
    cancelButton.hidden = !on;
    closeButton.disabled = on;
  };

  saveButton.addEventListener('click', async () => {
    setRunning(true);
    status.textContent = t('Saving…');
    try {
      const where = await saveImageAs(id, options());
      status.textContent = where ? t('Saved as {path}', { path: where }) : '';
      if (where) onSaved?.();
    } catch (error) {
      status.textContent = t("Couldn't save: {error}", { error: error?.message ?? error?.kind ?? String(error) });
    } finally {
      setRunning(false);
    }
  });

  convertButton.addEventListener('click', async () => {
    const items = others.map((f) => ({ path: f.path, edits: f.path === file.path ? getEdits() : undefined }));
    const rows = others.map((f) => h('li', {}, h('span', {}, f.name), h('span', { class: 'hint' }, t('Waiting'))));
    progress.replaceChildren(...rows);
    let done = 0;
    let failed = 0;
    setRunning(true);
    try {
      const started = await convertImages(items, options(), (e) => {
        if (e.event === 'started') status.textContent = t('Converting into {folder}…', { folder: e.folder });
        else if (e.event === 'done') {
          done++;
          rows[e.index].lastChild.textContent = '✓ ' + e.output.split(/[\\/]/).pop();
          rows[e.index].dataset.state = 'done';
        } else if (e.event === 'failed') {
          failed++;
          rows[e.index].lastChild.textContent = t("Couldn't convert: {error}", { error: e.error?.message ?? e.error?.kind });
          rows[e.index].dataset.state = 'failed';
        } else if (e.event === 'finished') {
          status.textContent = (e.cancelled ? t('Stopped.') + ' ' : '') + t('{done} converted, {failed} failed.', { done, failed });
          setRunning(false);
        }
      });
      if (!started) setRunning(false);
    } catch (error) {
      status.textContent = t("Couldn't convert: {error}", { error: error?.message ?? error?.kind ?? String(error) });
      setRunning(false);
    }
  });

  const dialog = h(
    'dialog',
    { class: 'img-dialog', 'aria-labelledby': 'img-dialog-title' },
    h('h2', { id: 'img-dialog-title' }, t('Save as / convert')),
    h('div', { class: 'field' }, h('label', { for: 'img-format' }, t('Format')), format),
    qualityRow,
    losslessRow,
    h('div', { class: 'field' }, h('label', { for: 'img-max' }, t('Longest side, in pixels (empty: keep the size)')), maxSide),
    h('div', { class: 'img-dialog-actions' }, saveButton, convertButton, cancelButton, closeButton),
    status,
    progress,
  );
  // Escape and the backdrop close it, but not while converting.
  dialog.addEventListener('cancel', (e) => running && e.preventDefault());
  dialog.addEventListener('close', () => dialog.remove());
  syncOptions();
  document.body.append(dialog);
  dialog.showModal();
}
