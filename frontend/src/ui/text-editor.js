// The text editor (stage 2): CodeMirror for Markdown, LaTeX and plain text; for Markdown, a preview
// rendered in Rust (jay_docs::markdown, already cleaned) that follows the editor's scroll.
// Saving keeps the file's encoding and line endings and won't overwrite changes made elsewhere
// (jay_docs::text); those are checked when the window gets focus and every few seconds.
import { EditorState } from '@codemirror/state';
import { EditorView, keymap, lineNumbers, highlightActiveLine, highlightActiveLineGutter, drawSelection } from '@codemirror/view';
import { defaultKeymap, history, historyKeymap, indentWithTab } from '@codemirror/commands';
import { syntaxHighlighting, HighlightStyle, Language, LanguageSupport, defineLanguageFacet } from '@codemirror/language';
import { parser as markdownParser, GFM } from '@lezer/markdown';
import { searchKeymap, highlightSelectionMatches } from '@codemirror/search';
import { tags } from '@lezer/highlight';
import { h } from './dom.js';
import { t } from '../core/i18n.js';
import { settings } from '../store/settings.js';
import { openText, fileStamp, saveText, renderMarkdown, exportHtml, sameStamp } from '../core/text.js';
import { openLink } from '../core/pdf.js';

/** jay_docs::text::Encoding as serialized. */
const ENCODING_NAMES = { utf8: 'UTF-8', 'utf8-bom': 'UTF-8 BOM', 'utf16-le': 'UTF-16 LE', 'utf16-be': 'UTF-16 BE', windows1252: 'Windows-1252' };
const CHECK_EVERY_MS = 3000;
// The Markdown grammar alone: @codemirror/lang-markdown would bring the HTML, CSS and JS modes too
// (~400 KB) for highlighting code blocks.
const markdown = () => new LanguageSupport(new Language(defineLanguageFacet(), markdownParser.configure([GFM]), [], 'markdown'));
const RENDER_DELAY_MS = 120;

// Colours from the app's theme tokens, so the editor follows Calm/Neon/Soft and light/dark.
const highlight = HighlightStyle.define([
  { tag: tags.heading, fontWeight: '700', color: 'var(--accent-text, var(--accent))' },
  { tag: tags.strong, fontWeight: '700' },
  { tag: tags.emphasis, fontStyle: 'italic' },
  { tag: tags.strikethrough, textDecoration: 'line-through' },
  { tag: [tags.link, tags.url], color: 'var(--accent)' },
  { tag: [tags.monospace, tags.quote, tags.comment, tags.processingInstruction, tags.contentSeparator], color: 'var(--text-muted)' },
]);
const theme = EditorView.theme({
  '&': { height: '100%', backgroundColor: 'var(--surface)', color: 'var(--text)', fontSize: '14px' },
  '.cm-scroller': { fontFamily: 'var(--font-mono)', lineHeight: '1.6' },
  '.cm-content': { caretColor: 'var(--accent)', padding: '12px 0' },
  '.cm-gutters': { backgroundColor: 'var(--surface)', color: 'var(--text-muted)', border: 'none' },
  '.cm-activeLine, .cm-activeLineGutter': { backgroundColor: 'color-mix(in srgb, var(--accent) 6%, transparent)' },
  '&.cm-focused .cm-selectionBackground, .cm-selectionBackground': { backgroundColor: 'color-mix(in srgb, var(--accent) 25%, transparent) !important' },
  '.cm-cursor': { borderLeftColor: 'var(--accent)' },
  '.cm-searchMatch': { backgroundColor: 'rgb(250 204 21 / 0.35)' },
  '.cm-panels': { backgroundColor: 'var(--surface-2)', color: 'var(--text)' },
});

/** Builds the editor for an opened text file; `el.isDirty()` and `el.dispose()` for files.js. */
export async function createTextEditor(file) {
  const root = h('div', { class: 'text-editor' });
  let doc;
  try {
    doc = await openText(file.path);
  } catch (error) {
    const message = error?.kind === 'toobig' ? t('This file is too big to edit (50 MB max).') : error?.kind === 'binary' ? t("This file isn't text.") : (error?.message ?? String(error));
    root.append(h('div', { class: 'view-notice' }, h('h2', {}, file.name), h('p', {}, t("Couldn't open it: {error}", { error: message }))));
    return root;
  }

  const isMarkdown = file.kind === 'markdown';
  let { encoding, lineEnding, stamp } = doc;
  let saved; // the document as last read or saved
  let force = false; // next save overwrites changes made elsewhere (the person chose to keep theirs)
  let saving = false; // our own write changes the stamp: don't take it for someone else's

  // ----- Status, banner and toolbar -----
  const dirtyMark = h('span', { class: 'editor-dirty', hidden: true, title: t('Not saved'), 'aria-label': t('Not saved') }, '●');
  const statusText = h('span', { class: 'hint' });
  const renderStatus = () => (statusText.textContent = `${ENCODING_NAMES[encoding] ?? encoding} · ${lineEnding === 'crlf' ? 'CRLF' : 'LF'}`);
  const banner = h('div', { class: 'editor-banner', role: 'alert', hidden: true });
  const showBanner = (text, ...buttons) => {
    banner.replaceChildren(h('span', {}, text), ...buttons);
    banner.hidden = false;
  };
  const hideBanner = () => (banner.hidden = true);
  const bannerButton = (label, onClick, secondary = false) => h('button', { type: 'button', class: secondary ? 'btn btn-secondary' : 'btn', onClick }, label);

  const saveButton = h('button', { type: 'button', class: 'btn', disabled: true, title: 'Ctrl+S', onClick: () => save() }, t('Save'));

  // ----- Editor -----
  const isDirty = () => !view.state.doc.eq(saved);
  const onChange = EditorView.updateListener.of((update) => {
    if (!update.docChanged) return;
    const dirty = isDirty();
    dirtyMark.hidden = !dirty;
    saveButton.disabled = !dirty;
    if (isMarkdown) scheduleRender();
  });
  const view = new EditorView({
    parent: h('div', { class: 'editor-pane' }),
    state: EditorState.create({
      doc: doc.text,
      extensions: [
        lineNumbers(),
        highlightActiveLineGutter(),
        highlightActiveLine(),
        drawSelection(),
        history(),
        highlightSelectionMatches(),
        EditorView.lineWrapping,
        EditorView.contentAttributes.of({ 'aria-label': file.name }),
        keymap.of([{ key: 'Mod-s', run: () => (save(), true) }, indentWithTab, ...defaultKeymap, ...historyKeymap, ...searchKeymap]),
        isMarkdown ? markdown() : [],
        syntaxHighlighting(highlight),
        theme,
        onChange,
      ],
    }),
  });
  saved = view.state.doc;
  const editorPane = view.dom.parentElement;

  // ----- Preview -----
  const preview = h('article', { class: 'markdown preview-pane', tabindex: '0', 'aria-label': t('Preview') });
  let renderTimer = 0;
  let renderToken = 0;
  async function render() {
    const token = ++renderToken;
    try {
      const html = await renderMarkdown(file.path, view.state.doc.toString());
      // Cleaned by ammonia in Rust (no scripts, no handlers); the CSP blocks inline scripts anyway.
      if (token === renderToken) preview.innerHTML = html;
      syncPreview();
    } catch (error) {
      preview.replaceChildren(h('p', { class: 'hint', 'data-tone': 'error' }, String(error?.message ?? error)));
    }
  }
  const scheduleRender = () => {
    clearTimeout(renderTimer);
    renderTimer = setTimeout(render, RENDER_DELAY_MS);
  };
  // Links in the preview never navigate the app: web and mail links open in the browser.
  preview.addEventListener('click', (e) => {
    const a = e.target.closest('a[href]');
    if (!a) return;
    e.preventDefault();
    if (/^(https?:|mailto:)/i.test(a.getAttribute('href'))) openLink(a.getAttribute('href')).catch(() => {});
  });

  /** Scrolls the preview to the block at the editor's top line (between markers, proportionally). */
  function syncPreview() {
    if (!isMarkdown || mode !== 'split') return;
    const top = view.lineBlockAtHeight(view.scrollDOM.scrollTop);
    const line = view.state.doc.lineAt(top.from).number;
    const markers = [...preview.querySelectorAll('[data-line]')];
    if (!markers.length) return;
    let i = markers.findLastIndex((m) => Number(m.dataset.line) <= line);
    if (i < 0) i = 0;
    const a = markers[i];
    const b = markers[i + 1];
    const aLine = Number(a.dataset.line);
    const fraction = b ? (line - aLine) / Math.max(Number(b.dataset.line) - aLine, 1) : 0;
    const aTop = a.offsetTop;
    const bTop = b ? b.offsetTop : preview.scrollHeight;
    preview.scrollTop = aTop + (bTop - aTop) * fraction - 12;
  }
  view.scrollDOM.addEventListener('scroll', () => requestAnimationFrame(syncPreview), { passive: true });

  // ----- Modes (Markdown only): code, split, preview -----
  let mode = isMarkdown ? (settings.get().markdownMode) : 'code';
  const body = h('div', { class: 'editor-body' }, editorPane, isMarkdown ? preview : null);
  const applyMode = () => {
    body.dataset.mode = mode;
    if (mode !== 'code') syncPreview();
  };
  const modeGroup = isMarkdown
    ? h(
        'div',
        { class: 'segmented', role: 'radiogroup', 'aria-label': t('View') },
        [
          ['code', t('Code')],
          ['split', t('Split')],
          ['preview', t('Preview')],
        ].map(([value, label]) =>
          h(
            'label',
            {},
            h('input', {
              type: 'radio',
              name: `mode-${file.path}`,
              value,
              checked: mode === value,
              onChange: () => {
                mode = value;
                settings.update({ markdownMode: value });
                applyMode();
              },
            }),
            label,
          ),
        ),
      )
    : null;

  // ----- Export (Markdown only) -----
  async function toHtml() {
    try {
      const where = await exportHtml(file.path, view.state.doc.toString(), file.name.replace(/\.[^.]+$/, ''));
      if (where) showBanner(t('Saved as {path}', { path: where }), bannerButton(t('OK'), hideBanner, true));
    } catch (error) {
      showBanner(t("Couldn't export: {error}", { error: error?.message ?? String(error) }), bannerButton(t('OK'), hideBanner, true));
    }
  }
  /** Prints only the preview; WebView2's print dialog has "Save as PDF". */
  async function toPdf() {
    await render();
    const printRoot = h('div', { id: 'print-root', class: 'markdown' });
    printRoot.innerHTML = preview.innerHTML;
    document.body.append(printRoot);
    const title = document.title;
    document.title = file.name.replace(/\.[^.]+$/, '');
    addEventListener(
      'afterprint',
      () => {
        printRoot.remove();
        document.title = title;
      },
      { once: true },
    );
    window.print();
  }
  const exportButtons = isMarkdown
    ? [h('button', { type: 'button', class: 'btn btn-secondary', onClick: toHtml }, t('Export HTML')), h('button', { type: 'button', class: 'btn btn-secondary', onClick: toPdf }, t('Export PDF'))]
    : [];

  // ----- Saving -----
  async function save() {
    if (!isDirty() && !force) return;
    const text = view.state.doc.toString();
    const snapshot = view.state.doc;
    saveButton.disabled = true;
    saving = true;
    try {
      stamp = await saveText(file.path, { text, encoding, lineEnding, stamp }, force);
      force = false;
      saved = snapshot;
      dirtyMark.hidden = !isDirty();
      saveButton.disabled = !isDirty();
      hideBanner();
    } catch (error) {
      saveButton.disabled = false;
      if (error?.kind === 'changed') {
        showBanner(
          t('{name} changed on disk since you opened it.', { name: file.name }),
          bannerButton(t('Overwrite with mine'), () => ((force = true), save())),
          bannerButton(t('Reload theirs'), reload, true),
        );
      } else if (error?.kind === 'unencodable') {
        showBanner(
          t('"{ch}" can\'t be saved in {encoding}.', { ch: error.ch, encoding: ENCODING_NAMES[encoding] }),
          bannerButton(t('Save as UTF-8'), () => {
            encoding = 'utf8';
            renderStatus();
            force = true; // same file, new encoding: save even if unchanged
            save();
          }),
          bannerButton(t('Cancel'), hideBanner, true),
        );
      } else {
        showBanner(t("Couldn't save: {error}", { error: error?.message ?? String(error) }), bannerButton(t('OK'), hideBanner, true));
      }
    } finally {
      saving = false;
    }
  }

  async function reload() {
    try {
      const fresh = await openText(file.path);
      ({ encoding, lineEnding, stamp } = fresh);
      view.dispatch({ changes: { from: 0, to: view.state.doc.length, insert: fresh.text } });
      saved = view.state.doc;
      dirtyMark.hidden = true;
      saveButton.disabled = true;
      force = false;
      renderStatus();
      hideBanner();
    } catch (error) {
      showBanner(t("Couldn't open it: {error}", { error: error?.message ?? String(error) }), bannerButton(t('OK'), hideBanner, true));
    }
  }

  // Changes made by another program: reload quietly if nothing here is unsaved, else ask.
  let asking = false;
  async function checkOutside() {
    if (!root.isConnected || asking || force || saving) return;
    let now;
    try {
      now = await fileStamp(file.path);
    } catch {
      return; // gone or locked for a moment; saving will say
    }
    if (saving || sameStamp(now, stamp)) return;
    if (!isDirty()) return reload();
    asking = true;
    showBanner(
      t('{name} changed on disk since you opened it.', { name: file.name }),
      bannerButton(t('Reload theirs'), () => ((asking = false), reload()), true),
      bannerButton(t('Keep mine'), () => {
        asking = false;
        force = true;
        hideBanner();
      }),
    );
  }
  const timer = setInterval(checkOutside, CHECK_EVERY_MS);
  addEventListener('focus', checkOutside);

  renderStatus();
  const toolbar = h(
    'div',
    { class: 'doc-toolbar', role: 'toolbar', 'aria-label': t('Editor tools') },
    modeGroup,
    file.kind === 'latex' ? h('span', { class: 'hint' }, t('The PDF preview of LaTeX arrives in stage {stage}.', { stage: 6 })) : null,
    h('span', { class: 'editor-status' }, dirtyMark, statusText),
    ...exportButtons,
    saveButton,
  );
  root.append(toolbar, banner, body);
  applyMode();
  if (isMarkdown) render();

  root.isDirty = isDirty;
  root.dispose = () => {
    clearInterval(timer);
    removeEventListener('focus', checkOutside);
    view.destroy();
  };
  queueMicrotask(() => view.focus());
  return root;
}

