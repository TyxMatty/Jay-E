// The LaTeX side of the text editor (stage 6): compiles with the TeX engine inside Jay-E
// (src-tauri/src/latex.rs, Tectonic's) and shows the PDF in the PDF viewer of stage 1, with TeX's
// errors as a list that jumps to the line.
import { h } from './dom.js';
import { t } from '../core/i18n.js';

const call = async (cmd, args) => (await import('@tauri-apps/api/core')).invoke(cmd, args);

/**
 * @param {object} file the opened .tex
 * @param {{ getText: () => string, isDirty: () => boolean, jumpTo: (line: number) => void }} editor
 */
export function createLatexPane(file, { getText, isDirty, jumpTo }) {
  const pane = h('div', { class: 'preview-pane latex-pane', 'aria-label': t('PDF') });
  const messages = h('div', { class: 'latex-messages', hidden: true, role: 'region', 'aria-label': t('Compile messages') });
  const status = h('span', { class: 'hint latex-status', role: 'status' });
  const compileButton = h('button', { type: 'button', class: 'btn', title: 'Ctrl+Enter' }, t('Compile'));
  const savePdfButton = h('button', { type: 'button', class: 'btn btn-secondary', disabled: true }, t('Save PDF…'));
  let viewer = null;
  let disposed = false;
  let token = 0;

  const notice = (...children) => pane.replaceChildren(h('div', { class: 'view-notice' }, ...children));

  function showMessages(diagnostics) {
    const shown = diagnostics.filter((d) => !d.warning || d.line);
    messages.hidden = shown.length === 0;
    messages.replaceChildren(
      ...shown.map((d) =>
        h(
          'button',
          { type: 'button', class: 'latex-message', 'data-tone': d.warning ? 'warning' : 'error', disabled: !d.line, onClick: () => d.line && jumpTo(d.line) },
          h('span', { class: 'latex-line' }, d.line ? t('Line {n}', { n: d.line }) : '—'),
          h('span', {}, d.message),
        ),
      ),
    );
  }

  async function compile() {
    if (disposed) return;
    const mine = ++token;
    status.textContent = t('Compiling… (the first time also downloads the TeX packages it uses)');
    try {
      const r = await call('latex_compile', { path: file.path, text: isDirty() ? getText() : null });
      if (mine !== token || disposed) return;
      const errors = r.diagnostics.filter((d) => !d.warning).length;
      status.textContent = errors ? t('{n} errors', { n: errors }) : t('Compiled in {s} s', { s: r.seconds.toFixed(1) });
      showMessages(r.diagnostics);
      if (r.pdf) {
        const old = viewer;
        viewer = await (await import('./pdf-viewer.js')).createPdfViewer(r.pdf);
        if (disposed) return viewer.dispose?.();
        pane.replaceChildren(viewer);
        old?.dispose?.();
        savePdfButton.disabled = false;
      } else if (!viewer) {
        notice(h('p', {}, t('No PDF came out: see the messages below the editor.')));
      }
    } catch (e) {
      if (mine !== token || e?.kind === 'cancelled') return;
      status.textContent = e?.kind === 'timeout' ? t('Stopped: compiling took more than 10 minutes.') : t("Couldn't compile: {error}", { error: e?.message ?? e?.kind ?? String(e) });
    }
  }

  compileButton.addEventListener('click', compile);
  savePdfButton.addEventListener('click', async () => {
    try {
      const where = await call('latex_save_pdf', { path: file.path });
      if (where) status.textContent = t('Saved as {path}', { path: where });
    } catch (e) {
      status.textContent = t("Couldn't save: {error}", { error: e?.message ?? e?.kind ?? String(e) });
    }
  });
  // Opening a .tex shows its PDF straight away (the first compile also fetches the TeX packages).
  compile();

  return {
    pane,
    messages,
    buttons: [status, compileButton, savePdfButton],
    compile,
    dispose() {
      disposed = true;
      call('latex_cancel', { path: file.path }).catch(() => {});
      viewer?.dispose?.();
    },
  };
}
