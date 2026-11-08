// The image editor (stage 3). The image stays decoded in Rust (src-tauri/src/image.rs); the page
// keeps the list of edits (rotate, flip, crop, adjust) and shows what Rust draws: a ≤ 2048 px
// preview of the whole image, plus, when zoomed past it, a sharp region of the full-size image
// for the part on screen. The original file is never changed: "Save as" writes a new one.
import { h } from './dom.js';
import { icons } from './icons.js';
import { t } from '../core/i18n.js';
import { openImage, closeImage, setEdits, imageUrls, imageHistogram, neutralAdjust, autoAdjust, curveAt, pickLut } from '../core/image.js';
import { openConvertDialog } from './image-convert.js';

const PREVIEW_SIDE = 2048; // keep in sync with image.rs
const PAD = 16;
const ZOOMS = [0.1, 0.25, 0.5, 0.75, 1, 1.5, 2, 3, 4, 8];
const noEdits = () => ({ ops: [], adjust: neutralAdjust(), strokes: [], areas: [] });
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
  // Perspective crop: the area outside the four corners dimmed, and a handle on each corner.
  const SVG_NS = 'http://www.w3.org/2000/svg';
  const quadShade = document.createElementNS(SVG_NS, 'path');
  quadShade.setAttribute('class', 'img-quad-shade');
  quadShade.setAttribute('fill-rule', 'evenodd');
  const quadSvg = document.createElementNS(SVG_NS, 'svg');
  quadSvg.setAttribute('class', 'img-quad');
  quadSvg.setAttribute('viewBox', '0 0 100 100');
  quadSvg.setAttribute('preserveAspectRatio', 'none');
  quadSvg.append(quadShade);
  const CORNERS = [t('Top-left corner'), t('Top-right corner'), t('Bottom-right corner'), t('Bottom-left corner')];
  const quadHandles = CORNERS.map((label, i) => h('div', { class: 'img-quad-handle', tabindex: '0', role: 'slider', 'aria-label': label, 'data-i': String(i) }));
  const quadLayer = h('div', { class: 'img-quad-layer', hidden: true }, quadSvg, ...quadHandles);
  const cropLayer = h('div', { class: 'img-crop', hidden: true }, cropBox, quadLayer);
  // Drawing: the stroke being drawn shows here until Rust's picture (with it in) arrives.
  const drawLayer = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
  drawLayer.setAttribute('class', 'img-draw-layer');
  drawLayer.setAttribute('hidden', '');
  const canvas = h('div', { class: 'img-canvas' }, base, region, cropLayer, drawLayer);
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
    next
      .decode()
      .then(() => {
        if (key !== baseKey) return;
        base.src = next.src;
        drawLayer.replaceChildren(); // the strokes are in the picture now
        base.style.transform = ''; // and the straightening
      })
      .catch(() => {});
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
    // The histogram follows the crop, rotations and flips (not the tone changes it is the base of).
    const shape = JSON.stringify(edits.ops);
    if (!adjustPanel.hidden && shape !== histShape) loadHistogram(shape);
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
  /** A change made by dragging (a slider, a curve point): live while it moves, one undo step. */
  let dragBefore = null;
  function live(mutate) {
    dragBefore ??= structuredClone(edits);
    mutate(edits.adjust);
    syncSliders();
    apply();
  }
  function settle() {
    if (dragBefore && !same(dragBefore, edits)) {
      undoStack.push(dragBefore);
      redoStack.length = 0;
    }
    dragBefore = null;
    renderButtons();
  }

  // ----- Adjust panel: light, color, levels with the histogram, the tone curve -----
  /** A slider: `get`/`set` read and write edits.adjust in the slider's own units. */
  function slider(key, label, { min = -100, max = 100, get, set } = {}) {
    get ??= (a) => Math.round(a[key] * 100);
    set ??= (a, v) => (a[key] = v / 100);
    const input = h('input', { type: 'range', min: String(min), max: String(max), step: '1', value: '0', id: `img-${key}-${id}` });
    const value = h('output', { class: 'hint', for: input.id }, '0');
    input.addEventListener('input', () => live((a) => set(a, Number(input.value))));
    // One undo step per drag, not per pixel moved.
    input.addEventListener('change', settle);
    return { input, value, get, row: h('div', { class: 'img-slider' }, h('label', { for: input.id }, label), input, value) };
  }
  const avg = (v) => (v[0] + v[1] + v[2]) / 3;
  const light = [
    slider('exposure', t('Exposure')),
    slider('brightness', t('Brightness')),
    slider('contrast', t('Contrast')),
    slider('highlights', t('Highlights')),
    slider('shadows', t('Shadows')),
  ];
  const color = [slider('temperature', t('Temperature')), slider('tint', t('Tint')), slider('vibrance', t('Vibrance')), slider('saturation', t('Saturation'))];
  const levelSliders = [
    slider('black', t('Black point'), { min: 0, max: 254, get: (a) => Math.round(avg(a.levels.black) * 255), set: (a, v) => (a.levels.black = [v / 255, v / 255, v / 255]) }),
    slider('gamma', t('Midtones'), { get: (a) => Math.round(Math.log2(a.levels.gamma) * 50), set: (a, v) => (a.levels.gamma = 2 ** (v / 50)) }),
    slider('white', t('White point'), { min: 1, max: 255, get: (a) => Math.round(avg(a.levels.white) * 255), set: (a, v) => (a.levels.white = [v / 255, v / 255, v / 255]) }),
  ];
  const filterSliders = [
    slider('denoise', t('Reduce noise'), { min: 0 }),
    slider('sharpen', t('Sharpen'), { min: 0 }),
    slider('blur', t('Blur'), { min: 0 }),
    slider('vignette', t('Vignette')),
  ];
  const sliders = [...light, ...color, ...levelSliders, ...filterSliders];
  // A color look from a .cube file: Rust opens the dialog and checks the table.
  const lutName = h('span', { class: 'hint img-lut-name' });
  const lutRemove = h('button', { type: 'button', class: 'icon-btn', title: t('Remove the color look'), 'aria-label': t('Remove the color look'), html: icons.close, onClick: () => change((e) => (e.adjust.lut = null)) });
  const lutStatus = h('span', { class: 'hint', role: 'status' });
  const lutButton = h('button', { type: 'button', class: 'btn btn-secondary', onClick: async () => {
    lutStatus.textContent = '';
    try {
      const picked = await pickLut();
      if (picked) change((e) => (e.adjust.lut = picked.path));
    } catch (error) {
      lutStatus.textContent = t("Couldn't use it: {error}", { error: error?.message ?? error?.kind ?? String(error) });
    }
  } }, t('Color look (.cube)…'));
  const grayscale = h('input', { type: 'checkbox', id: `img-bw-${id}` });
  grayscale.addEventListener('change', () => change((e) => (e.adjust.grayscale = grayscale.checked)));

  // The histogram (luma filled, the three channels as lines) under the levels.
  const histCanvas = h('canvas', { class: 'img-hist', width: '256', height: '72', role: 'img', 'aria-label': t('Histogram') });
  let hist = null;
  let histShape = null;
  async function loadHistogram(shape = JSON.stringify(edits.ops)) {
    histShape = shape;
    try {
      const next = await imageHistogram(id);
      if (shape !== histShape) return;
      hist = next;
      drawHistogram();
    } catch {
      /* the image was closed */
    }
  }
  function drawHistogram() {
    const g = histCanvas.getContext('2d');
    g.clearRect(0, 0, 256, 72);
    if (!hist) return;
    // Scaled to the 99th-tallest bin, so one spike (a white background) doesn't flatten the rest.
    const top = [...hist.l].sort((a, b) => b - a)[2] || 1;
    const css = getComputedStyle(histCanvas);
    const y = (n) => 72 - Math.min(n / top, 1) * 70;
    g.fillStyle = css.getPropertyValue('--text-muted') || '#888';
    g.globalAlpha = 0.45;
    g.beginPath();
    g.moveTo(0, 72);
    hist.l.forEach((n, v) => g.lineTo(v, y(n)));
    g.lineTo(255, 72);
    g.fill();
    g.globalAlpha = 0.8;
    for (const [key, stroke] of [['r', '#e5484d'], ['g', '#30a46c'], ['b', '#3e63dd']]) {
      g.strokeStyle = stroke;
      g.beginPath();
      hist[key].forEach((n, v) => (v ? g.lineTo(v, y(n)) : g.moveTo(v, y(n))));
      g.stroke();
    }
    g.globalAlpha = 1;
  }
  const autoButton = (mode, label) =>
    h('button', { type: 'button', class: 'btn btn-secondary', onClick: async () => {
      if (!hist) await loadHistogram();
      if (hist) change((e) => Object.assign(e.adjust, autoAdjust(hist, mode)));
    } }, label);

  // The tone curve: drag a point, click the line to add one, double-click or Delete to take one out.
  const SVG = 'http://www.w3.org/2000/svg';
  const svgEl = (tag, attrs) => {
    const el = document.createElementNS(SVG, tag);
    for (const [k, v] of Object.entries(attrs)) el.setAttribute(k, v);
    return el;
  };
  const curveLine = svgEl('path', { class: 'img-curve-line' });
  const curvePoints = svgEl('g', {});
  const curveSvg = svgEl('svg', { class: 'img-curve', viewBox: '-4 -4 108 108', role: 'group', 'aria-label': t('Tone curve: drag a point, click the line to add one') });
  curveSvg.append(svgEl('path', { class: 'img-curve-grid', d: 'M0 0H100V100H0Z M25 0V100 M50 0V100 M75 0V100 M0 25H100 M0 50H100 M0 75H100' }), svgEl('path', { class: 'img-curve-diag', d: 'M0 100L100 0' }), curveLine, curvePoints);
  const points = () => (edits.adjust.curve.length >= 2 ? edits.adjust.curve : [[0, 0], [1, 1]]);
  const toSvg = ([x, yv]) => [x * 100, 100 - yv * 100];
  function drawCurve() {
    const pts = points();
    curveLine.setAttribute('d', Array.from({ length: 101 }, (_, i) => `${i ? 'L' : 'M'}${i} ${100 - curveAt(pts, i / 100) * 100}`).join(''));
    curvePoints.replaceChildren(
      ...pts.map((p, i) => {
        const [cx, cy] = toSvg(p);
        const dot = svgEl('circle', { class: 'img-curve-point', cx, cy, r: '3.2', tabindex: '0', role: 'slider', 'aria-label': t('Curve point {n}', { n: i + 1 }), 'aria-valuetext': `${Math.round(p[0] * 255)} → ${Math.round(p[1] * 255)}` });
        dot.dataset.i = String(i);
        return dot;
      }),
    );
  }
  const fromEvent = (e) => {
    const r = curveSvg.getBoundingClientRect();
    const k = 108 / r.width;
    const clamp = (v) => Math.min(Math.max(v, 0), 1);
    return [clamp(((e.clientX - r.left) * k - 4) / 100), clamp(1 - ((e.clientY - r.top) * k - 4) / 100)];
  };
  /** Moves point `i` to `p`, kept between its neighbours (the ends keep their `in`). */
  function movePoint(i, [x, yv]) {
    live((a) => {
      const pts = points().map((q) => [...q]);
      const last = pts.length - 1;
      const lo = i === 0 ? 0 : pts[i - 1][0] + 0.01;
      const hi = i === last ? 1 : pts[i + 1][0] - 0.01;
      pts[i] = [i === 0 || i === last ? pts[i][0] : Math.min(Math.max(x, lo), hi), yv];
      a.curve = pts;
    });
  }
  curveSvg.addEventListener('pointerdown', (e) => {
    let i = e.target.dataset?.i != null ? Number(e.target.dataset.i) : null;
    if (i == null) {
      // A new point on the line, where it was clicked.
      const [x] = fromEvent(e);
      const pts = points().map((q) => [...q]);
      i = pts.findIndex((q) => q[0] > x);
      if (i <= 0) return;
      live((a) => (a.curve = [...pts.slice(0, i), [x, curveAt(pts, x)], ...pts.slice(i)]));
    }
    e.preventDefault();
    curveSvg.setPointerCapture(e.pointerId);
    const move = (ev) => movePoint(i, fromEvent(ev));
    const up = () => {
      curveSvg.removeEventListener('pointermove', move);
      curveSvg.removeEventListener('pointerup', up);
      settle();
      curvePoints.querySelector(`[data-i="${i}"]`)?.focus();
    };
    curveSvg.addEventListener('pointermove', move);
    curveSvg.addEventListener('pointerup', up);
  });
  const removePoint = (i) => {
    const pts = points();
    if (i > 0 && i < pts.length - 1) change((e) => (e.adjust.curve = pts.filter((_, j) => j !== i)));
  };
  curveSvg.addEventListener('dblclick', (e) => e.target.dataset?.i != null && removePoint(Number(e.target.dataset.i)));
  // Keyboard: arrows move the focused point, Delete takes it out.
  curveSvg.addEventListener('keydown', (e) => {
    const i = e.target.dataset?.i != null ? Number(e.target.dataset.i) : null;
    if (i == null) return;
    const step = e.shiftKey ? 0.05 : 0.01;
    const d = { ArrowLeft: [-step, 0], ArrowRight: [step, 0], ArrowUp: [0, step], ArrowDown: [0, -step] }[e.key];
    if (e.key === 'Delete' || e.key === 'Backspace') return e.preventDefault(), removePoint(i);
    if (!d) return;
    e.preventDefault();
    const p = points()[i];
    movePoint(i, [p[0] + d[0], Math.min(Math.max(p[1] + d[1], 0), 1)]);
    settle();
    curvePoints.querySelector(`[data-i="${i}"]`)?.focus();
  });

  function syncSliders() {
    for (const s of sliders) {
      s.input.value = String(s.get(edits.adjust));
      s.value.textContent = s.input.value;
    }
    grayscale.checked = edits.adjust.grayscale;
    lutName.textContent = edits.adjust.lut ? edits.adjust.lut.split(/[\\/]/).pop().replace(/\.cube$/i, '') : '';
    lutRemove.hidden = !edits.adjust.lut;
    drawCurve();
  }
  const section = (title, ...items) => h('div', { class: 'img-adjust-section', role: 'group', 'aria-label': title }, h('h3', {}, title), ...items);
  const resetAdjust = h('button', { type: 'button', class: 'btn btn-secondary', onClick: () => change((e) => (e.adjust = noEdits().adjust)) }, t('Reset'));
  const adjustPanel = h(
    'div',
    { class: 'img-adjust', hidden: true, role: 'group', 'aria-label': t('Adjust') },
    section(t('Light'), light.map((s) => s.row)),
    section(t('Color'), color.map((s) => s.row), h('label', { class: 'img-check' }, grayscale, t('Black and white'))),
    section(t('Levels'), histCanvas, levelSliders.map((s) => s.row), h('div', { class: 'img-auto' }, autoButton('tone', t('Auto tone')), autoButton('contrast', t('Auto contrast')), autoButton('color', t('Auto color')))),
    section(t('Filters'), filterSliders.map((s) => s.row), h('div', { class: 'img-lut' }, lutButton, lutName, lutRemove), lutStatus),
    section(t('Curve'), curveSvg, h('button', { type: 'button', class: 'btn btn-secondary', onClick: () => change((e) => (e.adjust.curve = [])) }, t('Straight line'))),
    h('div', { class: 'img-adjust-end' }, resetAdjust),
  );
  syncSliders();

  // ----- Crop -----
  let crop = null; // { x, y, w, h } in fractions of the edited image
  function showCropBox() {
    Object.assign(cropBox.style, { left: `${crop.x * 100}%`, top: `${crop.y * 100}%`, width: `${crop.w * 100}%`, height: `${crop.h * 100}%` });
  }
  // ----- Drawing and marking up: pen, highlighter, arrows and shapes, text, hiding an area -----
  const brush = { color: '#e5484d', size: 8, tool: 'pen' };
  let drawing = false;
  const colorInput = h('input', { type: 'color', value: brush.color, 'aria-label': t('Brush color'), title: t('Brush color') });
  colorInput.addEventListener('input', () => (brush.color = colorInput.value));
  const sizeInput = h('input', { type: 'range', min: '1', max: '60', value: String(brush.size), id: `img-brush-${id}` });
  sizeInput.addEventListener('input', () => (brush.size = Number(sizeInput.value)));
  const TOOLS = [
    ['pen', t('Pen')],
    ['highlighter', t('Highlighter')],
    ['arrow', t('Arrow')],
    ['rect', t('Rectangle')],
    ['ellipse', t('Ellipse')],
    ['text', t('Text')],
    ['blur', t('Blur an area')],
    ['pixelate', t('Pixelate an area')],
  ];
  const toolButtons = TOOLS.map(([tool, label]) => h('button', { type: 'button', class: 'btn btn-secondary', 'aria-pressed': String(tool === brush.tool), onClick: () => setTool(tool) }, label));
  function setTool(tool) {
    brush.tool = tool;
    TOOLS.forEach(([name], i) => toolButtons[i].setAttribute('aria-pressed', String(name === tool)));
  }
  const clearDrawing = h('button', { type: 'button', class: 'btn btn-secondary', onClick: () => (edits.strokes.length || edits.areas.length) && change((e) => ((e.strokes = []), (e.areas = []))) }, t('Clear drawing'));
  const drawPanel = h(
    'div',
    { class: 'img-draw', hidden: true, role: 'group', 'aria-label': t('Draw') },
    h('div', { class: 'img-draw-modes', role: 'group', 'aria-label': t('Brush') }, toolButtons),
    h('label', { class: 'img-draw-color' }, colorInput, t('Color')),
    h('div', { class: 'img-slider' }, h('label', { for: sizeInput.id }, t('Size')), sizeInput),
    clearDrawing,
    h('span', { class: 'hint' }, t('Drag on the picture to draw; click to write. Ctrl+Z takes back the last one.')),
  );
  function setDrawing(on) {
    drawing = on;
    if (on && crop) endCrop(false);
    drawPanel.hidden = !on;
    drawLayer.toggleAttribute('hidden', !on);
    renderButtons();
  }
  const SVGNS = 'http://www.w3.org/2000/svg';
  const round5 = (v) => Math.round(v * 1e5) / 1e5;
  /** A stroke made of these points (fractions), in the brush's color and size. */
  const strokeOf = (points, opacity = 1, scale = 1) => ({ at: edits.ops.length, color: brush.color, width: (brush.size * scale) / 1000, opacity, points: points.map(([x, y]) => [round5(x), round5(y)]) });
  /** The outline of a shape from corner `a` to corner `b` (fractions), as a line to draw. */
  function shapePoints(tool, a, b) {
    if (tool === 'rect') return [a, [b[0], a[1]], b, [a[0], b[1]], a];
    if (tool === 'ellipse') {
      const [cx, cy, rx, ry] = [(a[0] + b[0]) / 2, (a[1] + b[1]) / 2, Math.abs(b[0] - a[0]) / 2, Math.abs(b[1] - a[1]) / 2];
      return Array.from({ length: 73 }, (_, i) => [cx + rx * Math.cos((i / 72) * 2 * Math.PI), cy + ry * Math.sin((i / 72) * 2 * Math.PI)]);
    }
    // An arrow: the line, then its head, drawn in pixels so it isn't squashed on a long picture.
    const [ax, ay, bx, by] = [a[0] * w, a[1] * hgt, b[0] * w, b[1] * hgt];
    const angle = Math.atan2(by - ay, bx - ax);
    const head = Math.max((brush.size / 1000) * Math.max(w, hgt) * 4, Math.hypot(bx - ax, by - ay) * 0.18);
    const wing = (side) => [(bx - head * Math.cos(angle + side * 0.5)) / w, (by - head * Math.sin(angle + side * 0.5)) / hgt];
    return [a, b, wing(1), b, wing(-1)];
  }
  /** Writes `text` where it was clicked: drawn by the page (its font), laid on the picture by Rust. */
  function placeText(at) {
    const long = Math.max(w, hgt);
    const fontPx = Math.max(8, (brush.size / 1000) * long * 4);
    const input = h('input', { type: 'text', class: 'img-text-input', 'aria-label': t('Text'), style: `left:${at[0] * 100}%;top:${at[1] * 100}%;font-size:${fontPx * scale}px;color:${brush.color}` });
    canvas.append(input);
    input.focus();
    let done = false;
    const finish = (keep) => {
      if (done) return;
      done = true;
      const text = input.value.trim();
      input.remove();
      if (!keep || !text) return;
      const c = document.createElement('canvas');
      const g = c.getContext('2d');
      const font = `600 ${fontPx}px ${getComputedStyle(document.body).fontFamily}`;
      g.font = font;
      c.width = Math.max(1, Math.ceil(g.measureText(text).width + fontPx * 0.2));
      c.height = Math.ceil(fontPx * 1.3);
      g.font = font;
      g.fillStyle = brush.color;
      g.textBaseline = 'alphabetic';
      g.fillText(text, fontPx * 0.1, fontPx);
      const png = c.toDataURL('image/png').split(',')[1];
      change((e) => e.areas.push({ at: e.ops.length, x: round5(at[0]), y: round5(at[1]), w: round5(c.width / w), h: round5(c.height / hgt), kind: 'stamp', png }));
    };
    input.addEventListener('keydown', (e) => {
      if (e.key === 'Enter') finish(true);
      if (e.key === 'Escape') finish(false);
      e.stopPropagation();
    });
    input.addEventListener('blur', () => finish(true));
  }
  drawLayer.addEventListener('pointerdown', (e) => {
    if (e.button !== 0) return;
    e.preventDefault();
    const r = canvas.getBoundingClientRect();
    const frac = (ev) => [Math.min(Math.max((ev.clientX - r.left) / r.width, 0), 1), Math.min(Math.max((ev.clientY - r.top) / r.height, 0), 1)];
    const tool = brush.tool;
    if (tool === 'text') return void setTimeout(() => placeText(frac(e)));
    const long = Math.max(w, hgt);
    drawLayer.setAttribute('viewBox', `0 0 ${w} ${hgt}`);
    const path = document.createElementNS(SVGNS, 'path');
    const freehand = tool === 'pen' || tool === 'highlighter';
    const hiding = tool === 'blur' || tool === 'pixelate';
    // The highlighter is wider and lets the picture through; hiding shows a dashed box until Rust's picture comes.
    const stroke = freehand ? strokeOf([frac(e)], tool === 'highlighter' ? 0.4 : 1, tool === 'highlighter' ? 3 : 1) : null;
    const look = hiding ? { fill: 'rgb(0 0 0 / 0.25)', stroke: '#fff', 'stroke-width': String(long / 400), 'stroke-dasharray': `${long / 100} ${long / 150}` } : { fill: 'none', stroke: brush.color, 'stroke-width': String((stroke?.width ?? brush.size / 1000) * long), 'stroke-opacity': String(stroke?.opacity ?? 1), 'stroke-linecap': 'round', 'stroke-linejoin': 'round' };
    Object.entries(look).forEach(([k, v]) => path.setAttribute(k, v));
    const a = frac(e);
    let b = a;
    const d = (pts) => pts.map(([x, y], i) => `${i ? 'L' : 'M'}${x * w} ${y * hgt}`).join('') + (pts.length === 1 ? 'l0.01 0' : '');
    const redraw = () => path.setAttribute('d', freehand ? d(stroke.points) : hiding ? d([a, [b[0], a[1]], b, [a[0], b[1]], a]) : d(shapePoints(tool, a, b)));
    redraw();
    drawLayer.append(path);
    drawLayer.setPointerCapture(e.pointerId);
    let last = [e.clientX, e.clientY];
    const move = (ev) => {
      // A point every 2 screen pixels is smooth enough and keeps the list small.
      if (Math.hypot(ev.clientX - last[0], ev.clientY - last[1]) < 2) return;
      last = [ev.clientX, ev.clientY];
      if (freehand) stroke.points.push(frac(ev));
      else b = frac(ev);
      redraw();
    };
    const up = () => {
      drawLayer.removeEventListener('pointermove', move);
      drawLayer.removeEventListener('pointerup', up);
      drawLayer.removeEventListener('pointercancel', up);
      if (freehand) {
        stroke.points = stroke.points.map(([x, y]) => [round5(x), round5(y)]);
        return change((ed) => ed.strokes.push(stroke));
      }
      // A click without a drag makes nothing.
      if (Math.abs(b[0] - a[0]) * w < 3 && Math.abs(b[1] - a[1]) * hgt < 3) return path.remove();
      if (hiding) {
        const box = { at: edits.ops.length, x: round5(Math.min(a[0], b[0])), y: round5(Math.min(a[1], b[1])), w: round5(Math.abs(b[0] - a[0])), h: round5(Math.abs(b[1] - a[1])), kind: tool };
        return change((ed) => ed.areas.push(box));
      }
      change((ed) => ed.strokes.push(strokeOf(shapePoints(tool, a, b))));
    };
    drawLayer.addEventListener('pointermove', move);
    drawLayer.addEventListener('pointerup', up);
    drawLayer.addEventListener('pointercancel', up);
  });

  // The crop bar: a shape to keep, straightening, and the perspective crop.
  let ratio = null; // width ÷ height in pixels, or null for any shape
  const RATIOS = [['free', t('Free')], ['original', t('Original')], ['1:1', '1:1'], ['4:3', '4:3'], ['3:4', '3:4'], ['3:2', '3:2'], ['16:9', '16:9'], ['9:16', '9:16']];
  const ratioSelect = h('select', { class: 'input', id: `img-ratio-${id}` }, RATIOS.map(([value, label]) => h('option', { value }, label)));
  /** The crop area's height (in fractions) for a width (in fractions), keeping `ratio`. */
  const heightFor = (fw) => (fw * w) / (ratio * hgt);
  ratioSelect.addEventListener('change', () => {
    const v = ratioSelect.value;
    ratio = v === 'free' ? null : v === 'original' ? w / hgt : Number(v.split(':')[0]) / Number(v.split(':')[1]);
    if (!ratio || !crop) return;
    // The largest area of that shape, centered.
    let fw = 1;
    let fh = heightFor(1);
    if (fh > 1) [fw, fh] = [1 / fh, 1];
    crop = { x: (1 - fw) / 2, y: (1 - fh) / 2, w: fw, h: fh };
    showCropBox();
  });
  const straightenInput = h('input', { type: 'range', min: '-45', max: '45', step: '0.5', value: '0', id: `img-straighten-${id}` });
  const straightenValue = h('output', { class: 'hint', for: straightenInput.id }, '0°');
  /** How much a turn of `deg` shrinks the picture (the largest same-shaped part inside). */
  const shrink = (deg) => {
    const a = Math.abs((deg * Math.PI) / 180);
    return Math.min(w / (w * Math.cos(a) + hgt * Math.sin(a)), hgt / (w * Math.sin(a) + hgt * Math.cos(a)));
  };
  // The turn already in the list (the last operation), which straightening again replaces.
  const lastTurn = () => (edits.ops.at(-1)?.op === 'rotate' ? edits.ops.at(-1).degrees : 0);
  straightenInput.addEventListener('input', () => {
    const deg = Number(straightenInput.value);
    straightenValue.textContent = `${deg}°`;
    // Shown at once by turning the picture on screen; Rust's version follows on release.
    const rel = deg - lastTurn();
    base.style.transform = `rotate(${rel}deg) scale(${1 / shrink(rel)})`;
    region.hidden = true;
    canvas.classList.add('is-straightening');
  });
  straightenInput.addEventListener('change', () => {
    canvas.classList.remove('is-straightening');
    const deg = Number(straightenInput.value);
    if (deg === lastTurn()) return (base.style.transform = '');
    change((e) => {
      if (e.ops.at(-1)?.op === 'rotate') {
        if (deg === 0) e.ops.pop();
        else e.ops.at(-1).degrees = deg;
      } else e.ops.push({ op: 'rotate', degrees: deg });
    });
  });
  let quad = null; // the perspective corners, in fractions, or null
  const perspectiveButton = h('button', { type: 'button', class: 'btn btn-secondary', 'aria-pressed': 'false', onClick: () => setPerspective(!quad) }, t('Perspective'));
  function showQuad() {
    const pts = quad.map(([x, y]) => `${x * 100} ${y * 100}`);
    quadShade.setAttribute('d', `M0 0H100V100H0Z M${pts.join(' L')}Z`);
    quadHandles.forEach((el, i) => Object.assign(el.style, { left: `${quad[i][0] * 100}%`, top: `${quad[i][1] * 100}%` }));
  }
  function setPerspective(on) {
    quad = on ? [[0.1, 0.1], [0.9, 0.1], [0.9, 0.9], [0.1, 0.9]] : null;
    perspectiveButton.setAttribute('aria-pressed', String(on));
    quadLayer.hidden = !on;
    cropBox.hidden = on;
    ratioSelect.disabled = on;
    if (on) {
      showQuad();
      quadHandles[0].focus();
    } else cropBox.focus();
  }
  quadHandles.forEach((el, i) => {
    el.addEventListener('pointerdown', (e) => {
      e.preventDefault();
      e.stopPropagation();
      const r = canvas.getBoundingClientRect();
      el.setPointerCapture(e.pointerId);
      const move = (ev) => {
        quad[i] = [Math.min(Math.max((ev.clientX - r.left) / r.width, 0), 1), Math.min(Math.max((ev.clientY - r.top) / r.height, 0), 1)];
        showQuad();
      };
      const up = () => {
        el.removeEventListener('pointermove', move);
        el.removeEventListener('pointerup', up);
      };
      el.addEventListener('pointermove', move);
      el.addEventListener('pointerup', up);
    });
    // Arrows move the corner (Ctrl: further), Enter applies, Escape cancels.
    el.addEventListener('keydown', (e) => {
      const step = e.ctrlKey ? 0.05 : 0.005;
      const d = { ArrowLeft: [-step, 0], ArrowRight: [step, 0], ArrowUp: [0, -step], ArrowDown: [0, step] }[e.key];
      if (e.key === 'Enter') return e.preventDefault(), endCrop(true);
      if (e.key === 'Escape') return e.preventDefault(), endCrop(false);
      if (!d) return;
      e.preventDefault();
      quad[i] = [Math.min(Math.max(quad[i][0] + d[0], 0), 1), Math.min(Math.max(quad[i][1] + d[1], 0), 1)];
      showQuad();
    });
  });
  const cropPanel = h(
    'div',
    { class: 'img-draw img-crop-bar', hidden: true, role: 'group', 'aria-label': t('Crop') },
    h('div', { class: 'img-slider' }, h('label', { for: ratioSelect.id }, t('Shape')), ratioSelect),
    h('div', { class: 'img-slider' }, h('label', { for: straightenInput.id }, t('Straighten')), straightenInput, straightenValue),
    perspectiveButton,
    h('span', { class: 'hint' }, t('Drag on the picture for the area; Perspective: drag the four corners onto the page’s corners.')),
  );

  function startCrop() {
    if (drawing) setDrawing(false);
    crop = { x: 0.1, y: 0.1, w: 0.8, h: 0.8 };
    if (ratio) ratioSelect.dispatchEvent(new Event('change'));
    straightenInput.value = String(lastTurn());
    straightenValue.textContent = `${lastTurn()}°`;
    cropLayer.hidden = false;
    cropPanel.hidden = false;
    showCropBox();
    renderButtons();
    cropBox.focus();
  }
  function endCrop(applyIt) {
    if (applyIt && quad) pushOp({ op: 'perspective', corners: quad.map(([x, y]) => [Math.round(x * 1e5) / 1e5, Math.round(y * 1e5) / 1e5]) });
    else if (applyIt && crop && (crop.w < 0.999 || crop.h < 0.999)) pushOp({ op: 'crop', ...crop });
    if (quad) setPerspective(false);
    crop = null;
    cropLayer.hidden = true;
    cropPanel.hidden = true;
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
      let cw = Math.abs(x1 - x0);
      let ch = Math.abs(y1 - y0);
      if (ratio) {
        // The width leads; the height follows the shape, and both stop at the picture's edge.
        const room = y1 >= y0 ? 1 - y0 : y0;
        ch = heightFor(cw);
        if (ch > room) [cw, ch] = [(cw * room) / ch, room];
      }
      crop = { x: x1 >= x0 ? x0 : x0 - cw, y: y1 >= y0 ? y0 : y0 - ch, w: Math.max(cw, 0.002), h: Math.max(ch, 0.002) };
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
    if (e.shiftKey && ratio) {
      // One size for both sides, in the shape chosen.
      const grow = d[0] || -d[1];
      const fw = Math.min(Math.max(crop.w + grow, 0.01), 1 - crop.x, ((1 - crop.y) * ratio * hgt) / w);
      [crop.w, crop.h] = [fw, heightFor(fw)];
    } else if (e.shiftKey) {
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
    if (!adjustPanel.hidden) {
      sliders[0].input.focus();
      if (histShape !== JSON.stringify(edits.ops)) loadHistogram();
    }
  }, { 'aria-pressed': 'false' });
  const drawButton = iconButton(icons.pen, t('Draw'), () => setDrawing(!drawing), { 'aria-pressed': 'false' });
  const undoButton = iconButton(icons.undo, t('Undo'), undo, { disabled: true });
  const redoButton = iconButton(icons.redo, t('Redo'), redo, { disabled: true });
  const saveButton = h('button', { type: 'button', class: 'btn', onClick: () => openConvertDialog({ file, id, getEdits: () => edits, onSaved: () => ((savedEdits = structuredClone(edits)), renderButtons()) }) }, t('Save as…'));
  function renderButtons() {
    undoButton.disabled = !undoStack.length;
    redoButton.disabled = !redoStack.length;
    cropButton.setAttribute('aria-pressed', String(!!crop));
    applyCrop.hidden = !crop;
    for (const b of geometry) b.disabled = !!crop;
    drawButton.setAttribute('aria-pressed', String(drawing));
  }

  const toolbar = h(
    'div',
    { class: 'doc-toolbar', role: 'toolbar', 'aria-label': t('Image tools') },
    h('span', { class: 'img-tool-group' }, geometry, cropButton, applyCrop, adjustButton, drawButton),
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
  const root = h('div', { class: 'image-editor' }, toolbar, adjustPanel, drawPanel, cropPanel, stage);
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
