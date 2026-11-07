// The Convert section (stage 8): files of every kind in one list, one format per kind, converted
// one after another into a folder, with progress per file and a Stop button. Outputs that exist
// are asked about first (replace, keep both, skip). A file that fails fails alone.
import { h } from './dom.js';
import { icons } from './icons.js';
import { t } from '../core/i18n.js';
import { isTauri, pickFiles, formatSize } from '../core/files.js';
import { GROUPS, groupOf, outputOf, pickFolder, checkExisting, startBatch, cancelBatch, describeError } from '../core/convert.js';
import { settings } from '../store/settings.js';
import { openFiles } from './files.js';

export function createConvertPanel() {
  /** @type {Array<{ file: object, row: HTMLElement, status: HTMLElement, bar: HTMLProgressElement }>} */
  let queue = [];
  let running = false;
  const choices = { ...settings.get().convertTo };

  const list = h('ol', { class: 'cv-list', 'aria-label': t('Files to convert') });
  const formats = h('div', { class: 'cv-formats' });
  const status = h('p', { class: 'hint', role: 'status' });
  const addButton = h('button', { type: 'button', class: 'btn', disabled: !isTauri, onClick: async () => add(await pickFiles()) }, h('span', { class: 'btn-icon', html: icons.plus }), t('Add files…'));
  const addOpenButton = h('button', { type: 'button', class: 'btn btn-secondary', disabled: !isTauri, onClick: () => add(openFiles()) }, t('Add the open files'));
  const clearButton = h('button', { type: 'button', class: 'btn btn-secondary', onClick: () => setQueue([]) }, t('Clear'));
  const startButton = h('button', { type: 'button', class: 'btn', onClick: () => run() }, t('Convert…'));
  const stopButton = h('button', { type: 'button', class: 'btn btn-danger', hidden: true, onClick: () => cancelBatch().catch(() => {}) }, t('Stop'));
  const empty = h('p', { class: 'hint cv-empty' }, t('Add files of any kind Jay-E reads: they all go into one folder, each in the format chosen for its kind.'));

  function add(files) {
    const fresh = (files ?? []).filter((f) => !queue.some((q) => q.file.path === f.path));
    setQueue([...queue, ...fresh.map(entry)]);
  }

  function entry(file) {
    const status = h('span', { class: 'cv-status hint' });
    const bar = h('progress', { class: 'cv-bar', max: '1', hidden: true });
    const remove = h('button', { type: 'button', class: 'icon-btn', 'aria-label': t('Remove {name}', { name: file.name }), title: t('Remove'), html: icons.close, onClick: () => setQueue(queue.filter((q) => q.file !== file)) });
    const row = h('li', { class: 'cv-row' }, h('span', { class: 'cv-name', title: file.path }, file.name), h('span', { class: 'hint cv-size' }, formatSize(file.size)), h('span', { class: 'cv-to' }), status, bar, remove);
    return { file, row, status, bar, remove };
  }

  function setQueue(next) {
    queue = next;
    for (const q of queue) {
      if (q.row.dataset.state) continue;
      const to = outputOf(q.file, choices);
      q.row.querySelector('.cv-to').textContent = to ? `→ ${to.toUpperCase()}` : '';
      q.status.textContent = to ? '' : q.file.error ? q.file.error : t('Jay-E doesn’t convert this kind of file.');
      q.row.dataset.skip = to ? '' : '1';
    }
    list.replaceChildren(...queue.map((q) => q.row));
    empty.hidden = queue.length > 0;
    // One menu per kind in the list.
    formats.replaceChildren(
      ...GROUPS.filter((g) => queue.some((q) => groupOf(q.file) === g)).map((g) => {
        const id = `cv-to-${g.key}`;
        const select = h('select', { class: 'input', id, disabled: running }, g.to.map(([value, name]) => h('option', { value, selected: value === outputOf({ kind: g.kinds[0] }, choices) }, t(name))));
        select.addEventListener('change', () => {
          choices[g.key] = select.value;
          settings.update({ convertTo: { ...choices } });
          setQueue(queue);
        });
        return h('div', { class: 'field' }, h('label', { for: id }, t(g.name)), select);
      }),
    );
    startButton.disabled = running || !queue.some((q) => !q.row.dataset.skip);
    clearButton.disabled = running || !queue.length;
  }

  function setRunning(on) {
    running = on;
    addButton.disabled = addOpenButton.disabled = on || !isTauri;
    stopButton.hidden = !on;
    for (const q of queue) q.remove.disabled = on;
    setQueue(queue);
  }

  /** Replace, keep both or skip the outputs that exist; null if the person cancels. */
  function askExisting(count) {
    return new Promise((resolve) => {
      const choose = (value) => {
        resolve(value);
        dialog.close();
      };
      const dialog = h(
        'dialog',
        { class: 'img-dialog', 'aria-labelledby': 'cv-ask-title' },
        h('h2', { id: 'cv-ask-title' }, t('Some files already exist')),
        h('p', {}, count === 1 ? t('1 file with the same name is already in that folder.') : t('{count} files with the same names are already in that folder.', { count })),
        h(
          'div',
          { class: 'img-dialog-actions' },
          h('button', { type: 'button', class: 'btn', onClick: () => choose('rename') }, t('Keep both')),
          h('button', { type: 'button', class: 'btn btn-secondary', onClick: () => choose('replace') }, t('Replace')),
          h('button', { type: 'button', class: 'btn btn-secondary', onClick: () => choose('skip') }, t('Skip those')),
          h('button', { type: 'button', class: 'btn btn-secondary', onClick: () => choose(null) }, t('Cancel')),
        ),
      );
      dialog.addEventListener('close', () => {
        resolve(null);
        dialog.remove();
      });
      document.body.append(dialog);
      dialog.showModal();
    });
  }

  async function run() {
    // Converted or failed ones from a previous run start over.
    for (const q of queue) delete q.row.dataset.state;
    setQueue(queue);
    const jobs = queue.filter((q) => !q.row.dataset.skip);
    const items = jobs.map((q) => ({ path: q.file.path, to: outputOf(q.file, choices) }));
    try {
      const folder = await pickFolder();
      if (!folder) return;
      const taken = (await checkExisting(items)).filter(Boolean).length;
      const existing = taken ? await askExisting(taken) : 'rename';
      if (!existing) return;
      const counts = { done: 0, failed: 0, skipped: 0 };
      for (const q of jobs) {
        q.status.textContent = t('Waiting');
        q.row.dataset.state = 'waiting';
      }
      setRunning(true);
      status.textContent = t('Converting into {folder}…', { folder });
      await startBatch(items, existing, settings.get().imageExport.quality, (e) => {
        const q = jobs[e.index];
        if (e.event === 'started') {
          q.row.dataset.state = 'working';
          q.status.textContent = t('Converting…');
          q.bar.hidden = false;
          q.bar.removeAttribute('value');
        } else if (e.event === 'progress') {
          q.bar.value = e.done;
        } else if (e.event === 'done' || e.event === 'skipped' || e.event === 'failed') {
          q.bar.hidden = true;
          q.row.dataset.state = e.event;
          q.status.textContent = e.event === 'done' ? '✓ ' + e.output.split(/[\\/]/).pop() : e.event === 'skipped' ? t('Skipped: it already exists') : t("Couldn't convert: {error}", { error: describeError(e.error, t) });
          counts[e.event === 'done' ? 'done' : e.event]++;
        } else if (e.event === 'finished') {
          for (const j of jobs) if (j.row.dataset.state === 'waiting' || j.row.dataset.state === 'working') (j.status.textContent = t('Stopped')), (j.bar.hidden = true), delete j.row.dataset.state;
          status.textContent = (e.cancelled ? t('Stopped.') + ' ' : '') + t('{done} converted, {failed} failed, {skipped} skipped.', counts);
          setRunning(false);
        }
      });
    } catch (error) {
      status.textContent = t("Couldn't convert: {error}", { error: describeError(error, t) });
      setRunning(false);
    }
  }

  setQueue([]);
  return h(
    'section',
    { class: 'app-panel convert-panel' },
    h('h1', {}, t('Convert')),
    h('div', { class: 'cv-actions' }, addButton, addOpenButton, clearButton),
    empty,
    list,
    formats,
    h('div', { class: 'cv-actions' }, startButton, stopButton),
    status,
  );
}
