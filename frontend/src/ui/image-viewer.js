// The image editor (stage 3). The image stays decoded in Rust (src-tauri/src/image.rs); the page
// keeps the list of edits (rotate, flip, crop, adjust) and shows what Rust draws: a ≤ 2048 px
// preview of the whole image, plus, when zoomed past it, a sharp region of the full-size image
// for the part on screen. The original file is never changed: "Save as" writes a new one.
import { h } from './dom.js';
import { icons } from './icons.js';
import { t } from '../core/i18n.js';
import { openImage, closeImage, setEdits, imageUrls } from '../core/image.js';
import { openConvertDialog } from './image-convert.js';

const PREVIEW_SIDE = 2048; // keep in sync with image.rs
const PAD = 16;
const ZOOMS = [0.1, 0.25, 0.5, 0.75, 1, 1.5, 2, 3, 4, 8];
const noEdits = () => ({ ops: [], adjust: { brightness: 0, contrast: 0, saturation: 0 } });
const same = (a, b) => JSON.stringify(a) === JSON.stringify(b);

/** Resolves to the editor, or to `fallback()` when Rust can't decode the format (AVIF, SVG…). */
export async function createImageEditor(file, fallback) {
  let opened;
  try {
    opened = await openImage(file.path);
  } catch (error) {
    if (error?.kind === 'unsupported') return fallback();
    const message =
      error?.kind === 'toobig'
        ? t('This image is too big ({width} × {height}; 300 megapixels max).', { width: error.width, height: error.height })
        : (error?.message ?? String(error));
    return h('div', { class: 'view-notice' }, h('h2', {}, file.name), h('p', {}, t("Couldn't open it: {error}", { error: message })));
  }
  return buildEditor(file, opened);
}

function buildEditor(file, { id, width: origW, height: origH }) {
  const previewScale = Math.min(1, PREVIEW_SIDE / Math.max(origW, origH));
  let edits = noEdits();
  let savedEdits = noEdits();
  const undoStack = [];
  const redoStack = [];
  let rev = 0;
  let w = origW;
  let hgt = origH;
  /** 'fit' or CSS px per image px. */
  let zoom = 'fit';
  let scale = 1;
  let urls;

  // ----- Stage -----
  const base = h('img', { class: 'img-base', alt: file.name, draggable: 'false' });
  const region = h('img', { class: 'img-region', alt: '', draggable: 'false', hidden: true });
  const cropBox = h('div', { class: 'img-crop-box', tabindex: '0', role: 'group', 'aria-label': t('Crop area: arrows move it, Shift+arrows resize it') });
  const cropLayer = h('div', { class: 'img-crop', hidden: true }, cropBox);
  const canvas = h('div', { class: 'img-canvas' }, base, region, cropLayer);
  const stage = h('div', { class: 'img-stage', tabindex: '0', 'aria-label': file.name }, canvas);
  const sizeLabel = h('span', { class: 'hint' });
  const zoomLabel = h('span', { class: 'hint img-zoom-label', 'aria-live': 'polite' });

  function layout() {
    const fit = Math.min((stage.clientWidth - 2 * PAD) / w, (stage.clientHeight - 2 * PAD) / hgt, 1);
    scale = zoom === 'fit' ? Math.max(fit, 0.01) : zoom;
    canvas.style.width = `${w * scale}px`;
    canvas.style.height = `${hgt * scale}px`;
    zoomLabel.textContent = zoom === 'fit' ? `${t('Fit')} · ${Math.round(scale * 100)}%` : `${Math.round(scale * 100)}%`;
    sizeLabel.textContent = `${w} × ${hgt} px`;
    drawBase();
    scheduleRegion();
  }

  // The preview, at the size shown (in 256 px steps so zooming doesn't ask for every width).
  let baseKey = '';
  function drawBase() {
    if (!urls) return;
    const previewW = Math.max(1, Math.round(w * previewScale));
    const wanted = Math.min(previewW, Math.ceil((w * scale * devicePixelRatio) / 256) * 256);
    const key = `${rev}/${wanted}`;
    if (key === baseKey) return;
    baseKey = key;
    const next = new Image();
    next.src = urls.view(id, rev, wanted);
    // Swap once decoded: no flash of an empty image while the new one loads.
    next.decode().then(() => key === baseKey && (base.src = next.src)).catch(() => {});
  }

  // Zoomed past the preview: a sharp region of the full image for the visible part.
  let regionTimer = 0;
  const scheduleRegion = () => {
    clearTimeout(regionTimer);
    region.hidden = true;
    regionTimer = setTimeout(drawRegion, 150);
  };
  function drawRegion() {
    if (!urls || w * scale * devicePixelRatio <= w * previewScale * 1.05) return;
    const c = canvas.getBoundingClientRect();
    const s = stage.getBoundingClientRect();
    const left = Math.max(c.left, s.left) - c.left;
    const top = Math.max(c.top, s.top) - c.top;
    const right = Math.min(c.right, s.right) - c.left;
    const bottom = Math.min(c.bottom, s.bottom) - c.top;
    if (right <= left || bottom <= top) return;
    const [x, y, rw, rh] = [left / scale, top / scale, (right - left) / scale, (bottom - top) / scale].map(Math.floor);
    const key = `${rev}`;
    const next = new Image();
    next.src = urls.region(id, rev, x, y, Math.ceil(rw) + 1, Math.ceil(rh) + 1, (right - left) * devicePixelRatio);
    next.decode()
      .then(() => {
        if (key !== `${rev}`) return;
        region.src = next.src;
        Object.assign(region.style, { left: `${x * scale}px`, top: `${y * scale}px`, width: `${(Math.ceil(rw) + 1) * scale}px`, height: `${(Math.ceil(rh) + 1) * scale}px` });
        region.hidden = false;
      })
      .catch(() => {});
  }

  // ----- Edits -----
  let applyToken = 0;
  async function apply() {
    const token = ++applyToken;
    const result = await setEdits(id, edits);
    if (token !== applyToken) return;
    ({ rev, width: w, height: hgt } = result);
    layout();
    renderButtons();
  }
  /** Records the current edits for undo, then changes them. */
  function change(mutate) {
    undoStack.push(structuredClone(edits));
    redoStack.length = 0;
    mutate(edits);
    syncSliders();
    apply();
  }
  const pushOp = (op) => change((e) => e.ops.push(op));
  function undo() {
    if (!undoStack.length) return;
    redoStack.push(structuredClone(edits));
    edits = undoStack.pop();
    syncSliders();
    apply();
  }
  function redo() {
    if (!redoStack.length) return;
    undoStack.push(structuredClone(edits));
    edits = redoStack.pop();
    syncSliders();
    apply();
  }

  // ----- Adjust panel -----
  const sliders = ['brightness', 'contrast', 'saturation'].map((key) => {
    const label = { brightness: t('Brightness'), contrast: t('Contrast'), saturation: t('Saturation') }[key];
    const input = h('input', { type: 'range', min: '-100', max: '100', step: '1', value: '0', id: `img-${key}-${id}` });
    const value = h('output', { class: 'hint', for: input.id }, '0');
    let before = null; // the edits when this drag (or key press, or assistive change) started
    input.addEventListener('input', () => {
      before ??= structuredClone(edits);
      edits.adjust[key] = Number(input.value) / 100;
      value.textContent = input.value;
      apply();
    });
    // One undo step per drag, not per pixel moved.
    input.addEventListener('change', () => {
      if (before && !same(before, edits)) {
        undoStack.push(before);
        redoStack.length = 0;
      }
      before = null;
      renderButtons();
    });
    return { key, input, value, row: h('div', { class: 'img-slider' }, h('label', { for: input.id }, label), input, value) };
  });
  function syncSliders() {
    for (const s of sliders) {
      s.input.value = String(Math.round(edits.adjust[s.key] * 100));
      s.value.textContent = s.input.value;
    }
  }
  const resetAdjust = h('button', { type: 'button', class: 'btn btn-secondary', onClick: () => change((e) => (e.adjust = noEdits().adjust)) }, t('Reset'));
  const adjustPanel = h('div', { class: 'img-adjust', hidden: true, role: 'group', 'aria-label': t('Adjust') }, sliders.map((s) => s.row), resetAdjust);

  // ----- Crop -----
  let crop = null; // { x, y, w, h } in fractions of the edited image
  function showCropBox() {
    Object.assign(cropBox.style, { left: `${crop.x * 100}%`, top: `${crop.y * 100}%`, width: `${crop.w * 100}%`, height: `${crop.h * 100}%` });
  }
  function startCrop() {
    crop = { x: 0.1, y: 0.1, w: 0.8, h: 0.8 };
    cropLayer.hidden = false;
    showCropBox();
    renderButtons();
    cropBox.focus();
  }
  function endCrop(applyIt) {
    if (applyIt && crop && (crop.w < 0.999 || crop.h < 0.999)) pushOp({ op: 'crop', ...crop });
    crop = null;
    cropLayer.hidden = true;
    renderButtons();
    stage.focus();
  }
  // Drag on the image to draw the area.
  cropLayer.addEventListener('pointerdown', (e) => {
    const r = canvas.getBoundingClientRect();
    const fx = (v) => Math.min(Math.max((v - r.left) / r.width, 0), 1);
    const fy = (v) => Math.min(Math.max((v - r.top) / r.height, 0), 1);
    const x0 = fx(e.clientX);
    const y0 = fy(e.clientY);
    cropLayer.setPointerCapture(e.pointerId);
    const move = (ev) => {
      const x1 = fx(ev.clientX);
      const y1 = fy(ev.clientY);
      crop = { x: Math.min(x0, x1), y: Math.min(y0, y1), w: Math.max(Math.abs(x1 - x0), 0.002), h: Math.max(Math.abs(y1 - y0), 0.002) };
      showCropBox();
    };
    const up = () => {
      cropLayer.removeEventListener('pointermove', move);
      cropLayer.removeEventListener('pointerup', up);
      cropBox.focus();
    };
    cropLayer.addEventListener('pointermove', move);
    cropLayer.addEventListener('pointerup', up);
  });
  // The keyboard way: arrows move the area, Shift+arrows resize it, Enter applies, Escape cancels.
  cropBox.addEventListener('keydown', (e) => {
    const step = e.ctrlKey ? 0.1 : 0.01;
    const d = { ArrowLeft: [-step, 0], ArrowRight: [step, 0], ArrowUp: [0, -step], ArrowDown: [0, step] }[e.key];
    if (e.key === 'Enter') return e.preventDefault(), endCrop(true);
    if (e.key === 'Escape') return e.preventDefault(), endCrop(false);
    if (!d) return;
    e.preventDefault();
    if (e.shiftKey) {
      crop.w = Math.min(Math.max(crop.w + d[0], 0.01), 1 - crop.x);
      crop.h = Math.min(Math.max(crop.h + d[1], 0.01), 1 - crop.y);
    } else {
      crop.x = Math.min(Math.max(crop.x + d[0], 0), 1 - crop.w);
      crop.y = Math.min(Math.max(crop.y + d[1], 0), 1 - crop.h);
    }
    showCropBox();
  });

  // ----- Zoom -----
  function setZoom(next, anchor = null) {
    const r = stage.getBoundingClientRect();
    const ax = anchor ? anchor.x - r.left : stage.clientWidth / 2;
    const ay = anchor ? anchor.y - r.top : stage.clientHeight / 2;
    const c = canvas.getBoundingClientRect();
    // The image point under the anchor stays there.
    const px = (r.left + ax - c.left) / scale;
    const py = (r.top + ay - c.top) / scale;
    zoom = next;
    layout();
    const c2 = canvas.getBoundingClientRect();
    stage.scrollLeft += c2.left + px * scale - (r.left + ax);
    stage.scrollTop += c2.top + py * scale - (r.top + ay);
  }
  const zoomStep = (dir) => {
    const next = dir > 0 ? ZOOMS.find((z) => z > scale + 0.001) : [...ZOOMS].reverse().find((z) => z < scale - 0.001);
    if (next) setZoom(next);
  };

  // ----- Toolbar -----
  const iconButton = (icon, label, onClick, extra = {}) => h('button', { type: 'button', class: 'icon-btn', title: label, 'aria-label': label, html: icon, onClick, ...extra });
  const geometry = [
    iconButton(icons.rotateLeft, t('Rotate left'), () => pushOp({ op: 'rotateLeft' })),
    iconButton(icons.rotateRight, t('Rotate right'), () => pushOp({ op: 'rotateRight' })),
    iconButton(icons.flipH, t('Flip horizontally'), () => pushOp({ op: 'flipH' })),
    iconButton(icons.flipV, t('Flip vertically'), () => pushOp({ op: 'flipV' })),
  ];
  const cropButton = iconButton(icons.crop, t('Crop'), () => (crop ? endCrop(false) : startCrop()), { 'aria-pressed': 'false' });
  const applyCrop = h('button', { type: 'button', class: 'btn', hidden: true, onClick: () => endCrop(true) }, t('Apply crop'));
  const adjustButton = iconButton(icons.sliders, t('Adjust'), () => {
    adjustPanel.hidden = !adjustPanel.hidden;
    adjustButton.setAttribute('aria-pressed', String(!adjustPanel.hidden));
    if (!adjustPanel.hidden) sliders[0].input.focus();
  }, { 'aria-pressed': 'false' });
  const undoButton = iconButton(icons.undo, t('Undo'), undo, { disabled: true });
  const redoButton = iconButton(icons.redo, t('Redo'), redo, { disabled: true });
  const saveButton = h('button', { type: 'button', class: 'btn', onClick: () => openConvertDialog({ file, id, getEdits: () => edits, onSaved: () => ((savedEdits = structuredClone(edits)), renderButtons()) }) }, t('Save as…'));
  function renderButtons() {
    undoButton.disabled = !undoStack.length;
    redoButton.disabled = !redoStack.length;
    cropButton.setAttribute('aria-pressed', String(!!crop));
    applyCrop.hidden = !crop;
    for (const b of geometry) b.disabled = !!crop;
  }

  const toolbar = h(
    'div',
    { class: 'doc-toolbar', role: 'toolbar', 'aria-label': t('Image tools') },
    h('span', { class: 'img-tool-group' }, geometry, cropButton, applyCrop, adjustButton),
    h('span', { class: 'img-tool-group' }, undoButton, redoButton),
    h(
      'span',
      { class: 'img-tool-group img-zoom' },
      iconButton(icons.minus, t('Zoom out'), () => zoomStep(-1)),
      h('button', { type: 'button', class: 'btn btn-secondary', onClick: () => setZoom('fit') }, t('Fit')),
      h('button', { type: 'button', class: 'btn btn-secondary', onClick: () => setZoom(1) }, '100%'),
      iconButton(icons.plus, t('Zoom in'), () => zoomStep(1)),
      zoomLabel,
    ),
    sizeLabel,
    saveButton,
  );

  // ----- Events -----
  stage.addEventListener('scroll', scheduleRegion, { passive: true });
  stage.addEventListener(
    'wheel',
    (e) => {
      if (!e.ctrlKey) return;
      e.preventDefault();
      setZoom(Math.min(Math.max(scale * (e.deltaY < 0 ? 1.15 : 1 / 1.15), 0.02), 16), { x: e.clientX, y: e.clientY });
    },
    { passive: false },
  );
  const root = h('div', { class: 'image-editor' }, toolbar, adjustPanel, stage);
  root.addEventListener('keydown', (e) => {
    if (!e.ctrlKey || e.altKey || e.target.closest('input')) return;
    const actions = { z: undo, y: redo, '=': () => zoomStep(1), '+': () => zoomStep(1), '-': () => zoomStep(-1), 0: () => setZoom('fit'), 1: () => setZoom(1) };
    const action = actions[e.key.toLowerCase()];
    if (action) {
      e.preventDefault();
      action();
    }
  });
  new ResizeObserver(() => stage.isConnected && stage.clientWidth && layout()).observe(stage);

  imageUrls().then((u) => {
    urls = u;
    layout();
  });
  renderButtons();
  root.isDirty = () => !same(edits, savedEdits);
  root.dispose = () => closeImage(id).catch(() => {});
  return root;
}
