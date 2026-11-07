// The PDF viewer (stage 1). Every page has a placeholder of its real size, so the scrollbar is right
// from the start; only the pages near the screen are drawn (by Rust, src-tauri/src/pdf.rs) and the
// far ones give their bitmaps back. Text, links and search highlights are layers over the bitmap,
// positioned in points times --scale, so zooming only changes one CSS variable.
import { h } from './dom.js';
import { icons } from './icons.js';
import { t } from '../core/i18n.js';
import { openPdf, closePdf, setVisible, pageText, pageLinks, openLink, search, cancelSearch, fetchPage } from '../core/pdf.js';
import { createPdfTools } from './pdf-tools.js';

/** CSS pixels per point at 100%: a point is 1/72 in, a CSS pixel 1/96 in. */
const PX_PER_PT = 96 / 72;
const GAP = 12;
const PADDING = 16;
const ZOOMS = [0.25, 0.5, 0.75, 1, 1.25, 1.5, 2, 3, 4];
/** Pages kept drawn around the visible ones; farther ones free their bitmaps. */
const KEEP = 4;
const THUMB_WIDTH = 120;

/** Builds the viewer for an opened PDF file; `el.dispose()` closes it. */
export async function createPdfViewer(file) {
  const root = h('div', { class: 'pdf-viewer' });
  let disposed = false;
  root.dispose = () => {
    disposed = true;
  };
  // Editing state that outlives the rebuilds after each change (ui/pdf-tools.js).
  const editor = {};
  root.isDirty = () => !!editor.dirty;
  const rebuild = (edited) => {
    root.cleanupViewer?.();
    root.replaceChildren(...buildViewer(root, edited, editor, rebuild));
  };

  async function open(password) {
    try {
      const pdf = await openPdf(file.path, password);
      if (disposed) return closePdf(pdf.doc);
      root.replaceChildren(...buildViewer(root, pdf, editor, rebuild));
    } catch (error) {
      if (error === 'password') return root.replaceChildren(passwordForm(file, open, password != null));
      const message = error === 'invalid' ? t("This isn't a PDF Jay-E can read.") : error === 'toobig' ? t('This PDF is too big (1 GB max).') : (error?.io ?? String(error));
      root.replaceChildren(h('div', { class: 'view-notice' }, h('h2', {}, file.name), h('p', {}, t("Couldn't open it: {error}", { error: message }))));
    }
  }
  await open();
  return root;
}

function passwordForm(file, open, wrong) {
  const input = h('input', { class: 'input', type: 'password', id: 'pdf-password', autocomplete: 'off' });
  const form = h(
    'form',
    { class: 'view-notice pdf-password', onSubmit: (e) => (e.preventDefault(), open(input.value)) },
    h('h2', {}, file.name),
    h('p', {}, t('This PDF is protected by a password.')),
    h('div', { class: 'input-row' }, h('label', { class: 'visually-hidden', for: 'pdf-password' }, t('Password')), input, h('button', { type: 'submit', class: 'btn' }, t('Open'))),
    wrong ? h('p', { class: 'hint', 'data-tone': 'error', role: 'alert' }, t('Wrong password.')) : null,
  );
  queueMicrotask(() => input.focus());
  return form;
}

function buildViewer(root, { doc, pages: sizes }, editor, rebuild) {
  const count = sizes.length;
  const maxWidth = Math.max(...sizes.map(([w]) => w));
  /** 'width' | 'page' | a number (1 = 100%); after an edit, as it was. */
  let zoom = editor.view?.zoom ?? 'width';
  let scale = 1; // CSS px per point
  const tops = new Float64Array(count); // each page's top in CSS px, inside .pdf-pages
  let current = 0;

  // ----- Pages -----
  const pages = sizes.map(([w, hgt], index) => {
    const canvas = h('canvas', { class: 'pdf-canvas', 'aria-hidden': 'true' });
    const el = h('div', { class: 'pdf-page', role: 'img', 'aria-label': t('Page {n}', { n: index + 1 }) }, canvas);
    el.style.setProperty('--w', w);
    el.style.setProperty('--h', hgt);
    return { el, canvas, index, drawnWidth: 0, pending: 0, text: null, layers: false };
  });
  const sheet = h('div', { class: 'pdf-pages' }, pages.map((p) => p.el));
  const scroller = h('div', { class: 'pdf-scroll', tabindex: '0', 'aria-label': t('Pages') }, sheet);

  function layout() {
    const available = scroller.clientWidth - 2 * PADDING;
    if (zoom === 'width') scale = Math.max(available / maxWidth, 0.1);
    else if (zoom === 'page') {
      const [w, hgt] = sizes[current];
      scale = Math.max(Math.min(available / w, (scroller.clientHeight - 2 * PADDING) / hgt), 0.1);
    } else scale = zoom * PX_PER_PT;
    sheet.style.setProperty('--scale', scale);
    let y = PADDING;
    for (let i = 0; i < count; i++) {
      tops[i] = y;
      y += sizes[i][1] * scale + GAP;
    }
    zoomSelect.value = typeof zoom === 'string' ? zoom : String(zoom);
    zoomLabel.textContent = `${Math.round((scale / PX_PER_PT) * 100)}%`;
  }

  /** The page at a y inside .pdf-pages. */
  function pageAt(y) {
    let lo = 0;
    let hi = count - 1;
    while (lo < hi) {
      const mid = (lo + hi + 1) >> 1;
      if (tops[mid] <= y) lo = mid;
      else hi = mid - 1;
    }
    return lo;
  }

  // ----- Drawing -----
  let lastVisible = '';
  let drawTimer = 0;
  function update() {
    const top = scroller.scrollTop;
    const first = pageAt(top);
    const last = pageAt(top + scroller.clientHeight);
    const middle = pageAt(top + scroller.clientHeight / 3);
    if (middle !== current) {
      current = middle;
      pageInput.value = String(current + 1);
      thumbs.mark(current);
    }
    const key = `${first}-${last}`;
    if (key !== lastVisible) {
      lastVisible = key;
      setVisible(doc, first, last).catch(() => {});
    }
    for (const p of pages) {
      if (p.index >= first - 1 && p.index <= last + 1) draw(p);
      else if ((p.index < first - KEEP || p.index > last + KEEP) && p.drawnWidth) release(p);
      if (p.index >= first && p.index <= last) addLayers(p);
    }
  }
  // Scrolling draws at once; zooming waits for the gesture to settle (the old bitmaps stretch meanwhile).
  const scheduleDraw = (delay) => {
    clearTimeout(drawTimer);
    drawTimer = setTimeout(update, delay);
  };

  async function draw(p) {
    const width = Math.round(sizes[p.index][0] * scale * devicePixelRatio);
    if (p.drawnWidth === width || p.pending === width) return;
    p.pending = width;
    try {
      const bitmap = await fetchPage(doc, p.index, width);
      if (!bitmap) return;
      if (p.pending !== width) return bitmap.close();
      p.canvas.width = bitmap.width;
      p.canvas.height = bitmap.height;
      p.canvas.getContext('2d').drawImage(bitmap, 0, 0);
      bitmap.close();
      p.drawnWidth = width;
      p.el.classList.add('is-drawn');
    } catch {
      // Closed meanwhile, or the page failed: it stays blank, and is asked again when it comes back.
    } finally {
      if (p.pending === width) p.pending = 0;
    }
  }

  function release(p) {
    p.canvas.width = 0;
    p.canvas.height = 0;
    p.drawnWidth = 0;
    p.el.classList.remove('is-drawn');
  }

  // ----- Text, links and search highlights -----
  const measure = document.createElement('canvas').getContext('2d');
  async function addLayers(p) {
    if (p.layers) return;
    p.layers = true;
    const [lines, links] = await Promise.all([pageText(doc, p.index).catch(() => []), pageLinks(doc, p.index).catch(() => [])]);
    p.text = lines;
    // Transparent text over the bitmap, stretched to the same width, so selecting and copying work.
    const font = getComputedStyle(root).fontFamily;
    const textLayer = h(
      'div',
      { class: 'pdf-text' },
      lines.map((line) => {
        measure.font = `${line.h}px ${font}`;
        const natural = measure.measureText(line.text).width || 1;
        const span = h('span', {}, line.text);
        span.style.cssText = `--x:${line.x};--y:${line.y};--fs:${line.h};--sx:${line.w / natural}`;
        return span;
      }),
    );
    const linkLayer = h(
      'div',
      { class: 'pdf-links' },
      links.map(({ rect: [x, y, w, hgt], target }) => {
        const external = target.kind === 'uri';
        const a = h('a', {
          href: external ? target.uri : '#',
          title: external ? target.uri : t('Go to page {n}', { n: target.page + 1 }),
          'aria-label': external ? target.uri : t('Go to page {n}', { n: target.page + 1 }),
          onClick: (e) => {
            e.preventDefault();
            if (external) openLink(target.uri).catch(() => {});
            else goTo(target.page, target.top);
          },
        });
        a.style.cssText = `--x:${x};--y:${y};--w:${w};--h:${hgt}`;
        return a;
      }),
    );
    p.hitLayer = h('div', { class: 'pdf-hits', 'aria-hidden': 'true' });
    p.el.append(textLayer, p.hitLayer, linkLayer);
    paintHits(p);
    tools.layer(p);
  }

  // ----- Navigation and zoom -----
  /** Scrolls to a page, and to `top` points from its top when given. */
  function goTo(index, top = null) {
    const i = Math.min(Math.max(index, 0), count - 1);
    scroller.scrollTop = tops[i] - PADDING + (top != null ? top * scale : 0);
    update();
  }

  /** Zooms keeping the point at `anchorY` (px from the scroller's top) where it is. */
  function setZoom(next, anchorY = scroller.clientHeight / 3) {
    const y = scroller.scrollTop + anchorY;
    const page = pageAt(y);
    const within = (y - tops[page]) / scale; // points into that page
    zoom = next;
    layout();
    scroller.scrollTop = tops[page] + within * scale - anchorY;
    editor.view = { zoom, top: scroller.scrollTop };
    scheduleDraw(150);
  }

  const zoomStep = (dir) => {
    const now = scale / PX_PER_PT;
    const next = dir > 0 ? ZOOMS.find((z) => z > now + 0.01) : [...ZOOMS].reverse().find((z) => z < now - 0.01);
    if (next) setZoom(next);
  };

  // ----- Toolbar -----
  const pageInput = h('input', { class: 'input pdf-page-input', type: 'text', inputmode: 'numeric', value: '1', 'aria-label': t('Page number') });
  pageInput.addEventListener('change', () => {
    const n = parseInt(pageInput.value, 10);
    if (n >= 1 && n <= count) goTo(n - 1);
    else pageInput.value = String(current + 1);
  });
  pageInput.addEventListener('focus', () => pageInput.select());

  const zoomSelect = h(
    'select',
    { class: 'input pdf-zoom', 'aria-label': t('Zoom'), onChange: (e) => setZoom(e.target.value === 'width' || e.target.value === 'page' ? e.target.value : Number(e.target.value)) },
    h('option', { value: 'width' }, t('Fit width')),
    h('option', { value: 'page' }, t('Fit page')),
    ZOOMS.map((z) => h('option', { value: String(z) }, `${z * 100}%`)),
  );
  const zoomLabel = h('span', { class: 'hint pdf-zoom-label', 'aria-live': 'polite' });

  // ----- Search -----
  let hits = []; // [{ page, rect }] in document order
  let hitIndex = -1;
  let searched = '';
  const searchInput = h('input', { class: 'input pdf-search', type: 'search', placeholder: t('Search'), 'aria-label': t('Search in the PDF') });
  const hitsLabel = h('span', { class: 'hint pdf-hits-label', role: 'status' });

  function paintHits(p) {
    if (!p.hitLayer) return;
    p.hitLayer.replaceChildren(
      ...hits.flatMap((hit, i) => {
        if (hit.page !== p.index) return [];
        const mark = h('span', { class: i === hitIndex ? 'is-current' : '' });
        const [x, y, w, hgt] = hit.rect;
        mark.style.cssText = `--x:${x};--y:${y};--w:${w};--h:${hgt}`;
        return [mark];
      }),
    );
  }

  function showHit(i) {
    if (!hits.length) return;
    const before = hits[hitIndex]?.page;
    hitIndex = (i + hits.length) % hits.length;
    const hit = hits[hitIndex];
    // Bring it to a third of the way down, unless it's already in view.
    const y = tops[hit.page] + hit.rect[1] * scale;
    if (y < scroller.scrollTop || y > scroller.scrollTop + scroller.clientHeight - 40) scroller.scrollTop = y - scroller.clientHeight / 3;
    for (const page of new Set([before, hit.page])) if (page != null) paintHits(pages[page]);
    update();
    renderHitsLabel();
  }

  let searching = false;
  let searchedPages = 0;
  function renderHitsLabel() {
    if (!searched) return (hitsLabel.textContent = '');
    const progress = searching ? ` · ${t('page {n} of {count}', { n: searchedPages, count })}` : '';
    hitsLabel.textContent = hits.length ? `${hitIndex + 1} / ${hits.length}${progress}` : searching ? t('Searching…') + progress : t('No results');
  }

  function clearSearch() {
    cancelSearch(doc).catch(() => {});
    const touched = new Set(hits.map((x) => x.page));
    hits = [];
    hitIndex = -1;
    searched = '';
    searching = false;
    for (const page of touched) paintHits(pages[page]);
    renderHitsLabel();
  }

  function startSearch(query) {
    clearSearch();
    if (!query.trim()) return;
    searched = query;
    searching = true;
    searchedPages = 0;
    renderHitsLabel();
    search(doc, query, (e) => {
      if (searched !== query) return;
      if (e.event === 'hits') {
        for (const rect of e.rects) hits.push({ page: e.page, rect });
        paintHits(pages[e.page]);
        if (hitIndex < 0) showHit(0);
      } else {
        searchedPages = e.searched;
        if (e.event === 'done') searching = false;
      }
      renderHitsLabel();
    }).catch(() => {});
  }

  searchInput.addEventListener('keydown', (e) => {
    if (e.key === 'Enter') {
      e.preventDefault();
      if (searchInput.value !== searched) startSearch(searchInput.value);
      else showHit(hitIndex + (e.shiftKey ? -1 : 1));
    } else if (e.key === 'Escape') {
      searchInput.value = '';
      clearSearch();
      scroller.focus();
    }
  });
  searchInput.addEventListener('search', () => !searchInput.value && clearSearch());

  // ----- Thumbnails -----
  const thumbs = createThumbs(doc, sizes, (i) => goTo(i));
  const thumbsButton = h('button', { type: 'button', class: 'icon-btn', 'aria-pressed': 'false', title: t('Thumbnails'), 'aria-label': t('Thumbnails'), html: icons.sidebar });
  thumbsButton.addEventListener('click', () => {
    const show = thumbs.el.hidden;
    thumbs.el.hidden = !show;
    thumbsButton.setAttribute('aria-pressed', String(show));
    if (show) thumbs.mark(current, true);
  });

  const toolbar = h(
    'div',
    { class: 'doc-toolbar', role: 'toolbar', 'aria-label': t('PDF tools') },
    thumbsButton,
    h('span', { class: 'pdf-page-nav tb-group' }, pageInput, h('span', { class: 'hint' }, `/ ${count}`)),
    h('span', { class: 'pdf-search-box tb-group' }, searchInput, h('button', { type: 'button', class: 'icon-btn', title: t('Previous result'), 'aria-label': t('Previous result'), html: icons.up, onClick: () => showHit(hitIndex - 1) }), h('button', { type: 'button', class: 'icon-btn', title: t('Next result'), 'aria-label': t('Next result'), html: icons.down, onClick: () => showHit(hitIndex + 1) }), hitsLabel),
    h('span', { class: 'pdf-zoom-box tb-group' }, h('button', { type: 'button', class: 'icon-btn', title: t('Zoom out'), 'aria-label': t('Zoom out'), html: icons.minus, onClick: () => zoomStep(-1) }), zoomSelect, h('button', { type: 'button', class: 'icon-btn', title: t('Zoom in'), 'aria-label': t('Zoom in'), html: icons.plus, onClick: () => zoomStep(1) }), zoomLabel),
  );

  // ----- Events -----
  // A tab switch takes the viewer out of the page, which resets its scroll: remember it.
  let lastTop = 0;
  let framePending = false;
  scroller.addEventListener(
    'scroll',
    () => {
      lastTop = scroller.scrollTop;
      editor.view = { zoom, top: lastTop };
      // At most one update per frame, however many scroll events arrive.
      if (!framePending) {
        framePending = true;
        requestAnimationFrame(() => {
          framePending = false;
          update();
        });
      }
    },
    { passive: true },
  );
  scroller.addEventListener(
    'wheel',
    (e) => {
      if (!e.ctrlKey) return;
      e.preventDefault();
      const now = scale / PX_PER_PT;
      const next = Math.min(Math.max(now * (e.deltaY < 0 ? 1.1 : 1 / 1.1), 0.1), 8);
      setZoom(next, e.clientY - scroller.getBoundingClientRect().top);
    },
    { passive: false },
  );
  root.addEventListener('keydown', (e) => {
    if (!e.ctrlKey || e.altKey) return;
    if (tools.key(e)) return;
    const actions = { f: () => searchInput.focus(), '=': () => zoomStep(1), '+': () => zoomStep(1), '-': () => zoomStep(-1), 0: () => setZoom('width') };
    const action = actions[e.key];
    if (action) {
      e.preventDefault();
      action();
    }
  });
  let laidOut = false;
  new ResizeObserver(() => {
    if (!scroller.isConnected || !scroller.clientWidth) return;
    if (!laidOut) {
      // First time: draw at once (the first page is what opening a PDF waits for).
      laidOut = true;
      layout();
      // Rebuilt after an edit: back where it was.
      if (editor.view) scroller.scrollTop = editor.view.top;
      return update();
    }
    if (!scroller.scrollTop && lastTop) scroller.scrollTop = lastTop;
    // Keep the same spot when the window or the thumbnails change the width.
    setZoom(zoom, 0);
  }).observe(scroller);

  // ----- Editing -----
  const tools = createPdfTools({ doc, sizes, pages, scale: () => scale, scroller, editor, rebuild });

  const dispose = root.dispose;
  root.dispose = () => {
    dispose();
    thumbs.dispose();
    closePdf(doc).catch(() => {});
  };
  // Replaced by an edited version: this build's document is already closed by Rust.
  root.cleanupViewer = () => {
    thumbs.dispose();
    cancelSearch(doc).catch(() => {});
  };

  queueMicrotask(() => scroller.focus({ preventScroll: true }));
  return [toolbar, tools.bar, h('div', { class: 'pdf-body' }, thumbs.el, scroller, tools.organizer)];
}

/** The thumbnails column: drawn when scrolled into view, the current page marked. */
function createThumbs(doc, sizes, onPick) {
  const items = sizes.map(([w, hgt], i) => {
    const canvas = h('canvas', { 'aria-hidden': 'true' });
    canvas.style.aspectRatio = `${w} / ${hgt}`;
    const button = h('button', { type: 'button', class: 'pdf-thumb', 'data-index': i, 'aria-label': t('Page {n}', { n: i + 1 }), onClick: () => onPick(i) }, canvas, h('span', {}, String(i + 1)));
    return { button, canvas, drawn: false };
  });
  const el = h('nav', { class: 'pdf-thumbs', hidden: true, 'aria-label': t('Thumbnails') }, items.map((x) => x.button));
  const observer = new IntersectionObserver(
    (entries) => {
      for (const entry of entries) {
        if (!entry.isIntersecting) continue;
        const item = items[Number(entry.target.dataset.index)];
        if (item.drawn) continue;
        item.drawn = true;
        fetchPage(doc, Number(entry.target.dataset.index), THUMB_WIDTH * devicePixelRatio, { thumb: true })
          .then((b) => {
            if (!b) return (item.drawn = false);
            item.canvas.width = b.width;
            item.canvas.height = b.height;
            item.canvas.getContext('2d').drawImage(b, 0, 0);
            b.close();
          })
          .catch(() => (item.drawn = false));
      }
    },
    { root: el, rootMargin: '200px 0px' },
  );
  for (const item of items) observer.observe(item.button);
  let marked = -1;
  return {
    el,
    mark(i, scroll = false) {
      items[marked]?.button.removeAttribute('aria-current');
      items[i].button.setAttribute('aria-current', 'page');
      if ((scroll || marked !== i) && !el.hidden) items[i].button.scrollIntoView({ block: 'nearest' });
      marked = i;
    },
    dispose: () => observer.disconnect(),
  };
}
