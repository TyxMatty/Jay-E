// The video editor (stage 5). Playing and thumbnails are WebView2's <video> (hardware decoding);
// cutting and joining copy the streams in Rust (instant, lossless); converting and saving a frame
// go through Windows' Media Foundation. Results open in a new tab.
import { h } from './dom.js';
import { t } from '../core/i18n.js';
import { openVideoFiles } from './files.js';
import { clock } from '../core/files.js';

const core = () => import('@tauri-apps/api/core');
const call = async (cmd, args) => (await core()).invoke(cmd, args);
const THUMBS = 12;

export function createVideoEditor(file, url) {
  const video = h('video', { class: 'view-media', src: url, controls: true, preload: 'metadata', 'aria-label': file.name });
  let start = 0;
  let end = null; // null: the end of the video
  let info = { cuttable: false, keyFrames: [], frameRate: null, reason: null };
  // The razor: the video's pieces in time order, whether each is kept, and the order they're saved in.
  let pieces = [];
  let order = [];
  const duration = () => (Number.isFinite(video.duration) ? video.duration : 0);

  // ----- Strip: thumbnails, the marked part, the playhead -----
  const thumbs = Array.from({ length: THUMBS }, () => h('canvas', { width: '160', height: '90', 'aria-hidden': 'true' }));
  const range = h('div', { class: 'video-range' });
  const playhead = h('div', { class: 'video-playhead' });
  const piecesLayer = h('div', { class: 'video-pieces', 'aria-hidden': 'true' });
  const wave = h('canvas', { class: 'video-wave', width: '1200', height: '40', 'aria-hidden': 'true' });
  const strip = h('div', { class: 'video-strip', role: 'slider', tabindex: '0', 'aria-label': t('Position'), 'aria-valuemin': '0' }, h('div', { class: 'video-thumbs' }, thumbs), wave, piecesLayer, range, playhead);
  // Zoom: the strip wider than its box, scrolled.
  let zoom = 1;
  const stripScroll = h('div', { class: 'video-strip-scroll' }, strip);
  function setZoom(z) {
    zoom = Math.min(Math.max(z, 1), 32);
    strip.style.width = `${zoom * 100}%`;
    // Keep the playhead in view.
    const x = (video.currentTime / (duration() || 1)) * strip.scrollWidth;
    stripScroll.scrollLeft = x - stripScroll.clientWidth / 2;
    zoomLabel.textContent = `${zoom}×`;
  }
  stripScroll.addEventListener(
    'wheel',
    (e) => {
      if (!e.ctrlKey) return;
      e.preventDefault();
      setZoom(e.deltaY < 0 ? zoom * 2 : zoom / 2);
    },
    { passive: false },
  );
  const pct = (s) => `${(duration() ? (s / duration()) * 100 : 0).toFixed(3)}%`;
  function paintStrip() {
    const e = end ?? duration();
    range.style.left = pct(start);
    range.style.width = `calc(${pct(e)} - ${pct(start)})`;
    playhead.style.left = pct(video.currentTime);
    strip.setAttribute('aria-valuemax', String(Math.round(duration())));
    strip.setAttribute('aria-valuenow', String(Math.round(video.currentTime)));
    strip.setAttribute('aria-valuetext', clock(video.currentTime));
    rangeLabel.textContent = t('{start} to {end} · {length}', { start: clock(start), end: clock(e), length: clock(Math.max(e - start, 0)) });
    paintPieces();
  }
  // ----- Pieces (the razor) -----
  const current = () => pieces.find((p) => video.currentTime >= p.start && video.currentTime < p.end) ?? pieces.at(-1);
  const piecesLabel = h('span', { class: 'hint', 'aria-live': 'polite' });
  function paintPieces() {
    const now = current();
    const kept = order.filter((p) => p.keep);
    piecesLayer.replaceChildren(
      ...pieces.map((p) =>
        h(
          'div',
          { class: `video-piece${p.keep ? '' : ' is-dropped'}${p === now ? ' is-current' : ''}`, style: `left:${pct(p.start)};width:calc(${pct(p.end)} - ${pct(p.start)})` },
          p.keep && pieces.length > 1 ? h('span', { class: 'video-piece-n' }, String(kept.indexOf(p) + 1)) : null,
        ),
      ),
    );
    const length = kept.reduce((sum, p) => sum + p.end - p.start, 0);
    piecesLabel.textContent = pieces.length > 1 ? t('{kept} of {count} pieces kept · {length}', { kept: kept.length, count: pieces.length, length: clock(length) }) : '';
    saveButton.disabled = !info.cuttable || !kept.length || (pieces.length === 1 && kept.length === 1);
    for (const b of [dropButton, earlierButton, laterButton]) b.disabled = pieces.length < 2;
  }
  /** Splits the piece under the playhead there (nothing within a tenth of a second of its ends). */
  function split() {
    const p = current();
    const at = video.currentTime;
    if (!p || at - p.start < 0.1 || p.end - at < 0.1) return;
    const q = { start: at, end: p.end, keep: p.keep };
    p.end = at;
    pieces.splice(pieces.indexOf(p) + 1, 0, q);
    order.splice(order.indexOf(p) + 1, 0, q);
    paintPieces();
  }
  function toggleKeep() {
    const p = current();
    if (!p || pieces.length < 2) return;
    p.keep = !p.keep;
    paintPieces();
  }
  /** Moves the piece under the playhead earlier (-1) or later (+1) in the saved video. */
  function move(dir) {
    const p = current();
    const i = order.indexOf(p);
    // Past the dropped pieces: they aren't in the saved video, so trading places with one shows nothing.
    let j = i + dir;
    while (j >= 0 && j < order.length && !order[j].keep) j += dir;
    if (i < 0 || j < 0 || j >= order.length) return;
    [order[i], order[j]] = [order[j], order[i]];
    paintPieces();
  }
  async function savePieces() {
    const kept = order.filter((p) => p.keep);
    setStatus(t('Saving the pieces…'));
    try {
      const saved = await call('video_splice', { path: file.path, pieces: kept.map((p) => [p.start, p.end]) });
      setStatus(saved ? t('Saved as {path}', { path: saved[0] }) : '');
    } catch (e) {
      setStatus(t("Couldn't save: {error}", { error: errorText(e) }));
    }
  }
  /** One frame back or forward (paused). */
  function stepFrame(dir) {
    video.pause();
    const fps = info.frameRate || 30;
    video.currentTime = Math.min(Math.max(video.currentTime + dir / fps, 0), duration());
  }
  // The sound's waveform under the thumbnails.
  async function drawWave() {
    const peaks = await call('video_peaks', { path: file.path, bins: wave.width });
    const g = wave.getContext('2d');
    g.clearRect(0, 0, wave.width, wave.height);
    if (!peaks.length) return;
    g.fillStyle = getComputedStyle(strip).getPropertyValue('--accent') || '#8b5cf6';
    const mid = wave.height / 2;
    peaks.forEach((v, x) => {
      const hgt = Math.max(1, v * wave.height);
      g.fillRect(x, mid - hgt / 2, 1, hgt);
    });
  }
  strip.addEventListener('pointerdown', (e) => {
    const r = strip.getBoundingClientRect();
    video.currentTime = ((e.clientX - r.left) / r.width) * duration();
  });
  strip.addEventListener('keydown', (e) => {
    const step = { ArrowLeft: -1, ArrowRight: 1, PageDown: -10, PageUp: 10 }[e.key];
    if (step === undefined) return;
    e.preventDefault();
    video.currentTime = Math.min(Math.max(video.currentTime + step, 0), duration());
  });
  // Thumbnails from a second, hidden player: seek, draw, next.
  async function drawThumbs() {
    const probe = h('video', { src: url, muted: true, preload: 'auto' });
    await new Promise((r) => probe.addEventListener('loadedmetadata', r, { once: true }));
    for (let i = 0; i < THUMBS; i++) {
      probe.currentTime = ((i + 0.5) / THUMBS) * probe.duration;
      await new Promise((r) => probe.addEventListener('seeked', r, { once: true }));
      const c = thumbs[i];
      const ctx = c.getContext('2d');
      const scale = Math.max(c.width / probe.videoWidth, c.height / probe.videoHeight);
      const w = probe.videoWidth * scale;
      const hh = probe.videoHeight * scale;
      ctx.drawImage(probe, (c.width - w) / 2, (c.height - hh) / 2, w, hh);
    }
    probe.removeAttribute('src');
    probe.load();
  }

  // ----- Marks and actions -----
  const rangeLabel = h('span', { class: 'hint video-range-label', 'aria-live': 'polite' });
  const status = h('span', { class: 'hint doc-status', role: 'status' });
  const setStatus = (text) => (status.textContent = text);
  const markStart = () => {
    start = Math.min(video.currentTime, end ?? Infinity);
    paintStrip();
  };
  const markEnd = () => {
    end = Math.max(video.currentTime, start);
    paintStrip();
  };
  const errorText = (e) => e?.message ?? e?.what ?? (e?.kind === 'mismatch' ? t('{file} has different settings (codec or size): convert it first.', { file: e.file }) : e?.kind) ?? String(e);

  const cutButton = h('button', { type: 'button', class: 'btn', disabled: true, title: t('Without re-encoding: instant, no quality loss. Starts at the key frame at or before the start.') }, t('Cut'));
  cutButton.addEventListener('click', async () => {
    setStatus(t('Cutting…'));
    try {
      const saved = await call('video_cut', { path: file.path, start, end: end ?? duration() });
      setStatus(saved ? t('Saved {start} to {end} (from the key frame) as {path}', { start: clock(saved.start), end: clock(saved.end), path: saved.path }) : '');
    } catch (e) {
      setStatus(t("Couldn't cut: {error}", { error: errorText(e) }));
    }
  });

  const button = (label, onClick, extra = {}) => h('button', { type: 'button', class: 'btn btn-secondary', onClick, ...extra }, label);
  const zoomLabel = h('span', { class: 'hint' }, '1×');
  const dropButton = button(t('Drop / keep piece'), toggleKeep, { title: 'Delete', disabled: true });
  const earlierButton = button(t('Move earlier'), () => move(-1), { title: 'Alt+←', disabled: true });
  const laterButton = button(t('Move later'), () => move(1), { title: 'Alt+→', disabled: true });
  const saveButton = h('button', { type: 'button', class: 'btn', disabled: true, onClick: savePieces, title: t('The kept pieces, in their order, without re-encoding.') }, t('Save pieces…'));
  const razorBar = h(
    'div',
    { class: 'doc-toolbar video-razor', role: 'toolbar', 'aria-label': t('Pieces') },
    h('span', { class: 'img-tool-group' }, button(t('◀ Frame'), () => stepFrame(-1), { title: ',' }), button(t('Frame ▶'), () => stepFrame(1), { title: '.' })),
    h('span', { class: 'img-tool-group' }, button(t('Split here'), split, { title: 'S' }), dropButton, earlierButton, laterButton, saveButton),
    piecesLabel,
    h('span', { class: 'img-tool-group' }, button('−', () => setZoom(zoom / 2), { 'aria-label': t('Zoom out the timeline') }), zoomLabel, button('+', () => setZoom(zoom * 2), { 'aria-label': t('Zoom in the timeline') })),
  );
  const toolbar = h(
    'div',
    { class: 'doc-toolbar', role: 'toolbar', 'aria-label': t('Video tools') },
    h('span', { class: 'img-tool-group' }, button(t('Start here'), markStart, { title: 'I' }), button(t('End here'), markEnd, { title: 'O' }), rangeLabel),
    status,
    h(
      'span',
      { class: 'img-tool-group' },
      cutButton,
      button(t('Join…'), () => openJoinDialog(file)),
      button(t('Edit and export'), () => toggleEdit(), { 'aria-pressed': 'false', class: 'btn btn-secondary video-edit-toggle' }),
      button(t('Convert…'), () => openConvertDialog(file)),
      button(t('Save frame'), async () => {
        try {
          const where = await call('video_frame', { path: file.path, seconds: video.currentTime });
          if (where) setStatus(t('Saved as {path}', { path: where }));
        } catch (e) {
          setStatus(t("Couldn't save the frame: {error}", { error: errorText(e) }));
        }
      }),
    ),
  );

  video.addEventListener('loadedmetadata', () => {
    pieces = [{ start: 0, end: duration(), keep: true }];
    order = [...pieces];
    paintStrip();
    drawThumbs().catch(() => {});
    drawWave().catch(() => {});
  });
  video.addEventListener('timeupdate', paintStrip);
  video.addEventListener('seeked', paintStrip);
  call('video_info', { path: file.path })
    .then((i) => {
      info = i;
      cutButton.disabled = !info.cuttable;
      paintPieces();
      if (!info.cuttable) cutButton.title = t('Only MP4 and MOV can be cut without re-encoding: convert first.');
    })
    .catch(() => {});

  // ----- Edit and export: the picture, speed, sound, text and a logo, previewed on the player -----
  const look = { turns: 0, flipH: false, flipV: false, shape: 'original', brightness: 0, contrast: 0, saturation: 0, speed: 1, volume: 1, mute: false, fadeIn: 0, fadeOut: 0 };
  /** Text and pictures laid on: { from, to, area (fractions of the edited frame, a PNG stamp), src }. */
  const overlays = [];
  const turner = h('div', { class: 'video-turner' }, video);
  const overlayLayer = h('div', { class: 'video-overlays', 'aria-hidden': 'true' });
  const frameBox = h('div', { class: 'video-frame' }, turner, overlayLayer);
  const stage = h('div', { class: 'video-stage' }, frameBox);
  const SHAPES = [['original', t('Original')], ['1:1', '1:1'], ['9:16', '9:16'], ['16:9', '16:9'], ['4:5', '4:5']];
  const transformed = () => look.turns % 4 || look.flipH || look.flipV || look.shape !== 'original';
  /** The crop (fractions of the turned frame) for the chosen shape: centered, as large as fits. */
  function cropFor(rw, rh) {
    if (look.shape === 'original') return null;
    const [a, b] = look.shape.split(':').map(Number);
    const want = a / b;
    let [cw, ch] = rw / rh > want ? [(want * rh) / rw, 1] : [1, rw / (want * rh)];
    return { x: (1 - cw) / 2, y: (1 - ch) / 2, w: cw, h: ch };
  }
  /** Lays the player out as the export will look: turned, flipped and cut to the shape, fitted. */
  function fitFrame() {
    const [vw, vh] = [video.videoWidth || 16, video.videoHeight || 9];
    const odd = look.turns % 2 === 1;
    const [rw, rh] = odd ? [vh, vw] : [vw, vh];
    const crop = cropFor(rw, rh) ?? { x: 0, y: 0, w: 1, h: 1 };
    const [ew, eh] = [rw * crop.w, rh * crop.h];
    const s = Math.min(stage.clientWidth / ew, stage.clientHeight / eh) || 1;
    Object.assign(frameBox.style, { width: `${ew * s}px`, height: `${eh * s}px` });
    Object.assign(turner.style, { width: `${rw * s}px`, height: `${rh * s}px`, left: `${-crop.x * rw * s}px`, top: `${-crop.y * rh * s}px` });
    Object.assign(video.style, {
      width: `${vw * s}px`,
      height: `${vh * s}px`,
      left: `${((rw - vw) * s) / 2}px`,
      top: `${((rh - vh) * s) / 2}px`,
      transform: `scale(${look.flipH ? -1 : 1}, ${look.flipV ? -1 : 1}) rotate(${look.turns * 90}deg)`,
      filter: `brightness(${1 + look.brightness}) contrast(${1 + look.contrast}) saturate(${1 + look.saturation})`,
    });
    // The player's own controls would turn with the picture: the bar's Play does it then.
    video.controls = !transformed();
    playButton.hidden = !transformed();
    return { rw, rh, crop, ew, eh };
  }
  new ResizeObserver(() => stage.isConnected && fitFrame()).observe(stage);
  video.addEventListener('loadedmetadata', fitFrame);
  function applyLook() {
    video.playbackRate = look.speed;
    video.preservesPitch = false; // as the export: the pitch goes with the speed
    video.volume = Math.min(look.volume, 1);
    video.muted = look.mute;
    fitFrame();
    showOverlays();
  }
  function showOverlays() {
    const now = video.currentTime;
    overlayLayer.replaceChildren(
      ...overlays
        .filter((o) => now >= o.from && now < o.to)
        .map((o) => h('img', { src: o.src, alt: '', style: `left:${o.area.x * 100}%;top:${o.area.y * 100}%;width:${o.area.w * 100}%;height:${o.area.h * 100}%` })),
    );
  }
  video.addEventListener('timeupdate', showOverlays);
  video.addEventListener('seeked', showOverlays);
  const playButton = button(t('Play / pause'), () => (video.paused ? video.play() : video.pause()), { hidden: true, title: t('Space') });

  /** The edits for Rust: the image editor's operations and color, the overlays, speed and sound. */
  function renderEdits() {
    const { rw, rh, crop } = fitFrame();
    void rw;
    void rh;
    const ops = [...Array(look.turns % 4)].map(() => ({ op: 'rotateRight' }));
    if (look.flipH) ops.push({ op: 'flipH' });
    if (look.flipV) ops.push({ op: 'flipV' });
    if (crop.w < 1 || crop.h < 1) ops.push({ op: 'crop', ...crop });
    return {
      picture: { ops, adjust: { brightness: look.brightness, contrast: look.contrast, saturation: look.saturation }, strokes: [], areas: [] },
      overlays: overlays.map(({ from, to, area }) => ({ from, to, area: { ...area, at: ops.length } })),
      speed: look.speed,
      volume: look.volume,
      mute: look.mute,
      fadeIn: look.fadeIn,
      fadeOut: look.fadeOut,
    };
  }
  const editSlider = (key, label, min, max, scale) => {
    const input = h('input', { type: 'range', min: String(min), max: String(max), step: '1', value: String(look[key] * scale), id: `video-${key}` });
    const out = h('output', { class: 'hint', for: input.id }, String(look[key] * scale));
    input.addEventListener('input', () => {
      look[key] = Number(input.value) / scale;
      out.textContent = input.value;
      applyLook();
    });
    return h('div', { class: 'img-slider' }, h('label', { for: input.id }, label), input, out);
  };
  const shapeSelect = h('select', { class: 'input', id: 'video-shape', onChange: () => ((look.shape = shapeSelect.value), applyLook()) }, SHAPES.map(([v, l]) => h('option', { value: v }, l)));
  const speedSelect = h('select', { class: 'input', id: 'video-speed', onChange: () => ((look.speed = Number(speedSelect.value)), applyLook()) }, [0.25, 0.5, 0.75, 1, 1.25, 1.5, 2, 4].map((v) => h('option', { value: String(v), selected: v === 1 }, `${v}×`)));
  const mute = h('input', { type: 'checkbox', id: 'video-mute', onChange: () => ((look.mute = mute.checked), applyLook()) });
  const seconds = (key, label) => {
    const input = h('input', { type: 'number', class: 'input video-seconds', min: '0', max: '10', step: '0.5', value: '0', id: `video-${key}` });
    input.addEventListener('input', () => (look[key] = Math.min(Math.max(Number(input.value) || 0, 0), 10)));
    return h('div', { class: 'img-slider' }, h('label', { for: input.id }, label), input);
  };
  // Text: drawn here (the app's font, white with a dark edge), laid on by Rust for the marked part.
  const textInput = h('input', { type: 'text', class: 'input', id: 'video-text', placeholder: t('Text to show') });
  const textPlace = h('select', { class: 'input', 'aria-label': t('Where') }, [['bottom', t('Bottom')], ['middle', t('Middle')], ['top', t('Top')]].map(([v, l]) => h('option', { value: v }, l)));
  const overlayList = h('ul', { class: 'video-overlay-list' });
  function listOverlays() {
    overlayList.replaceChildren(
      ...overlays.map((o, i) =>
        h('li', {}, h('span', {}, `${o.label} · ${clock(o.from)}–${clock(o.to)}`), h('button', { type: 'button', class: 'icon-btn', 'aria-label': t('Remove {name}', { name: o.label }), html: '×', onClick: () => (overlays.splice(i, 1), listOverlays(), showOverlays()) })),
      ),
    );
  }
  function addText() {
    const text = textInput.value.trim();
    if (!text) return;
    const { ew, eh } = fitFrame();
    const fontPx = Math.max(16, eh * 0.06);
    const c = document.createElement('canvas');
    const g = c.getContext('2d');
    const font = `700 ${fontPx}px ${getComputedStyle(document.body).fontFamily}`;
    g.font = font;
    c.width = Math.ceil(g.measureText(text).width + fontPx * 0.6);
    c.height = Math.ceil(fontPx * 1.4);
    g.font = font;
    g.lineJoin = 'round';
    g.lineWidth = fontPx * 0.15;
    g.strokeStyle = 'rgba(0,0,0,0.75)';
    g.fillStyle = '#fff';
    g.strokeText(text, fontPx * 0.3, fontPx * 1.05);
    g.fillText(text, fontPx * 0.3, fontPx * 1.05);
    const w = Math.min(c.width / ew, 0.96);
    const hh = (w * ew * (c.height / c.width)) / eh;
    const y = textPlace.value === 'top' ? 0.05 : textPlace.value === 'middle' ? (1 - hh) / 2 : 0.95 - hh;
    const src = c.toDataURL('image/png');
    overlays.push({ label: text, from: start, to: end ?? duration(), src, area: { x: (1 - w) / 2, y, w, h: hh, kind: 'stamp', png: src.split(',')[1] } });
    textInput.value = '';
    listOverlays();
    showOverlays();
  }
  async function addPicture() {
    const picked = await call('video_pick_picture');
    if (!picked) return;
    const [png, pw, ph] = picked;
    const { ew, eh } = fitFrame();
    const w = 0.18;
    const hh = (w * ew * (ph / pw)) / eh;
    overlays.push({ label: t('Picture'), from: 0, to: duration(), src: `data:image/png;base64,${png}`, area: { x: 0.97 - w, y: 0.97 - hh, w, h: hh, kind: 'stamp', png } });
    listOverlays();
    showOverlays();
  }
  // Export: a preset, or a size to shrink to.
  const PRESETS = {
    original: { maxHeight: null, videoKbps: 8000, audioKbps: 160 },
    whatsapp: { maxHeight: 720, videoKbps: 1500, audioKbps: 128 },
    youtube: { maxHeight: 1080, videoKbps: 8000, audioKbps: 192 },
    small: { maxHeight: 480, videoKbps: 800, audioKbps: 96 },
  };
  const presetSelect = h(
    'select',
    { class: 'input', id: 'video-preset' },
    [['original', t('Best quality')], ['whatsapp', t('WhatsApp (720p)')], ['youtube', t('YouTube (1080p)')], ['small', t('Small file (480p)')], ['size', t('Shrink to a size…')], ['audio', t('Sound only (M4A)')]].map(([v, l]) => h('option', { value: v }, l)),
  );
  const sizeInput = h('input', { type: 'number', class: 'input video-seconds', min: '1', max: '4000', step: '1', value: '25', id: 'video-size', 'aria-label': t('Size in MB') });
  const sizeRow = h('div', { class: 'img-slider', hidden: true }, h('label', { for: 'video-size' }, t('Size in MB (Discord: 25, WhatsApp: 16)')), sizeInput);
  presetSelect.addEventListener('change', () => (sizeRow.hidden = presetSelect.value !== 'size'));
  /** The encoder settings for the preset; for a size, the bit rate that fits it. */
  function exportOptions() {
    const p = presetSelect.value;
    if (p === 'audio') return { audioOnly: true, maxHeight: null, videoKbps: 1000, audioKbps: 160 };
    if (p !== 'size') return { audioOnly: false, ...PRESETS[p] };
    const seconds = Math.max(duration() / look.speed, 1);
    const audioKbps = 96;
    // 5% for the container; never below what still looks like a video.
    const videoKbps = Math.max(150, Math.floor(((Number(sizeInput.value) || 25) * 8 * 1024 * 0.95) / seconds - audioKbps));
    return { audioOnly: false, maxHeight: videoKbps < 600 ? 480 : videoKbps < 1500 ? 720 : null, videoKbps, audioKbps };
  }
  const exportBar = h('progress', { max: '1', value: '0', hidden: true, class: 'video-progress' });
  const exportStatus = h('span', { class: 'hint', role: 'status' });
  const exportButton = h('button', { type: 'button', class: 'btn' }, t('Export…'));
  const stopButton = h('button', { type: 'button', class: 'btn btn-danger', hidden: true, onClick: () => call('video_cancel').catch(() => {}) }, t('Stop'));
  exportButton.addEventListener('click', async () => {
    const { Channel } = await core();
    const channel = new Channel();
    const running = (on) => ((exportButton.disabled = on), (stopButton.hidden = !on), (exportBar.hidden = !on));
    channel.onmessage = (e) => {
      if (e.event === 'progress') {
        exportBar.value = e.done;
        exportStatus.textContent = `${Math.round(e.done * 100)}%`;
      } else {
        running(false);
        exportStatus.textContent = e.event === 'finished' ? t('Saved as {path}', { path: e.path }) : e.error?.kind === 'cancelled' ? t('Stopped.') : t("Couldn't export: {error}", { error: errorText(e.error) });
      }
    };
    running(true);
    exportStatus.textContent = '';
    try {
      if (!(await call('video_render', { path: file.path, options: exportOptions(), edits: renderEdits(), channel }))) running(false);
    } catch (e) {
      running(false);
      exportStatus.textContent = t("Couldn't export: {error}", { error: errorText(e) });
    }
  });
  const section = (title, ...items) => h('div', { class: 'img-adjust-section', role: 'group', 'aria-label': title }, h('h3', {}, title), ...items);
  const editPanel = h(
    'div',
    { class: 'img-adjust video-edit', hidden: true, role: 'group', 'aria-label': t('Edit and export') },
    section(
      t('Picture'),
      h(
        'div',
        { class: 'img-auto' },
        button(t('Rotate'), () => ((look.turns = (look.turns + 1) % 4), applyLook())),
        button(t('Flip'), () => ((look.flipH = !look.flipH), applyLook())),
        button(t('Upside down'), () => ((look.flipV = !look.flipV), applyLook())),
      ),
      h('div', { class: 'img-slider' }, h('label', { for: 'video-shape' }, t('Shape')), shapeSelect),
      editSlider('brightness', t('Brightness'), -100, 100, 100),
      editSlider('contrast', t('Contrast'), -100, 100, 100),
      editSlider('saturation', t('Saturation'), -100, 100, 100),
    ),
    section(
      t('Speed and sound'),
      h('div', { class: 'img-slider' }, h('label', { for: 'video-speed' }, t('Speed')), speedSelect),
      editSlider('volume', t('Volume'), 0, 200, 100),
      h('label', { class: 'img-check' }, mute, t('Mute')),
      seconds('fadeIn', t('Fade in (s)')),
      seconds('fadeOut', t('Fade out (s)')),
    ),
    section(
      t('Text and picture'),
      h('div', { class: 'img-auto' }, textInput, textPlace, button(t('Add text'), addText)),
      h('span', { class: 'hint' }, t('Shown in the marked part (Start here / End here), or all along.')),
      button(t('Add a picture (logo)…'), addPicture),
      overlayList,
    ),
    section(t('Export'), h('div', { class: 'img-slider' }, h('label', { for: 'video-preset' }, t('As')), presetSelect), sizeRow, h('div', { class: 'img-auto' }, exportButton, stopButton), exportBar, exportStatus),
  );
  function toggleEdit() {
    editPanel.hidden = !editPanel.hidden;
    toolbar.querySelector('.video-edit-toggle')?.setAttribute('aria-pressed', String(!editPanel.hidden));
  }
  razorBar.prepend(playButton);
  const root = h('div', { class: 'video-editor' }, toolbar, editPanel, stage, razorBar, stripScroll);
  root.addEventListener('keydown', (e) => {
    if (e.ctrlKey || e.target.closest('input, select')) return;
    if (e.altKey && (e.key === 'ArrowLeft' || e.key === 'ArrowRight')) return e.preventDefault(), move(e.key === 'ArrowLeft' ? -1 : 1);
    if (e.altKey) return;
    if (e.key === 'i' || e.key === 'I') markStart();
    else if (e.key === 'o' || e.key === 'O') markEnd();
    else if (e.key === 's' || e.key === 'S') split();
    else if (e.key === 'Delete') toggleKeep();
    else if (e.key === ' ' && !video.controls) e.preventDefault(), video.paused ? video.play() : video.pause();
    else if (e.key === ',') stepFrame(-1);
    else if (e.key === '.') stepFrame(1);
  });
  return root;
}

function dialogShell(title, ...children) {
  const dialog = h('dialog', { class: 'img-dialog', 'aria-label': title }, h('h2', {}, title), ...children);
  dialog.addEventListener('close', () => dialog.remove());
  document.body.append(dialog);
  dialog.showModal();
  return dialog;
}

function openJoinDialog(file) {
  // Every open MP4/MOV, in tab order; this one checked first.
  const videos = openVideoFiles().filter((f) => ['mp4', 'mov'].includes(f.format));
  const boxes = videos.map((f) => h('input', { type: 'checkbox', checked: true, value: f.path }));
  const status = h('p', { class: 'hint', role: 'status' });
  const joinButton = h('button', { type: 'button', class: 'btn' }, t('Join'));
  const dialog = dialogShell(
    t('Join videos'),
    h('p', { class: 'hint' }, t('In tab order, without re-encoding. They must have the same codec, size and settings (parts of one recording, or cuts of one file).')),
    h('ol', { class: 'img-progress' }, videos.map((f, i) => h('li', {}, h('label', { class: 'img-check' }, boxes[i], f.name)))),
    h('div', { class: 'img-dialog-actions' }, joinButton, h('button', { type: 'button', class: 'btn btn-secondary', onClick: () => dialog.close() }, t('Close'))),
    status,
  );
  joinButton.addEventListener('click', async () => {
    const paths = boxes.filter((b) => b.checked).map((b) => b.value);
    if (paths.length < 2) return (status.textContent = t('Pick at least two videos.'));
    joinButton.disabled = true;
    status.textContent = t('Joining…');
    try {
      const where = await call('video_join', { paths });
      status.textContent = where ? t('Saved as {path}', { path: where }) : '';
    } catch (e) {
      status.textContent = t("Couldn't join: {error}", { error: e?.kind === 'mismatch' ? t('{file} has different settings (codec or size): convert it first.', { file: e.file }) : (e?.message ?? e?.kind ?? String(e)) });
    } finally {
      joinButton.disabled = false;
    }
  });
  void file;
}

function openConvertDialog(file) {
  const kind = h('select', { class: 'input', id: 'video-kind' }, h('option', { value: 'video' }, t('Video MP4 (H.264 + AAC)')), h('option', { value: 'audio' }, t('Audio only (M4A)')));
  const height = h('select', { class: 'input', id: 'video-height' }, [['', t('Original')], ['2160', '2160p'], ['1080', '1080p'], ['720', '720p'], ['480', '480p'], ['360', '360p']].map(([v, l]) => h('option', { value: v }, l)));
  const quality = h('select', { class: 'input', id: 'video-quality' }, [['8000', t('High')], ['4000', t('Medium')], ['1500', t('Small file')]].map(([v, l]) => h('option', { value: v, selected: v === '4000' }, l)));
  const bar = h('progress', { max: '1', value: '0', hidden: true, class: 'video-progress' });
  const status = h('p', { class: 'hint', role: 'status' });
  const goButton = h('button', { type: 'button', class: 'btn' }, t('Convert'));
  const stopButton = h('button', { type: 'button', class: 'btn btn-danger', hidden: true, onClick: () => call('video_cancel').catch(() => {}) }, t('Stop'));
  const closeButton = h('button', { type: 'button', class: 'btn btn-secondary', onClick: () => dialog.close() }, t('Close'));
  const videoRows = [h('div', { class: 'field' }, h('label', { for: 'video-height' }, t('Height')), height), h('div', { class: 'field' }, h('label', { for: 'video-quality' }, t('Quality')), quality)];
  kind.addEventListener('change', () => videoRows.forEach((r) => (r.hidden = kind.value === 'audio')));
  let running = false;
  const dialog = dialogShell(
    t('Convert video'),
    h('div', { class: 'field' }, h('label', { for: 'video-kind' }, t('Format')), kind),
    videoRows,
    h('div', { class: 'img-dialog-actions' }, goButton, stopButton, closeButton),
    bar,
    status,
  );
  dialog.addEventListener('cancel', (e) => running && e.preventDefault());
  const setRunning = (on) => {
    running = on;
    goButton.disabled = closeButton.disabled = on;
    stopButton.hidden = !on;
    bar.hidden = !on;
  };
  goButton.addEventListener('click', async () => {
    const { Channel } = await core();
    const channel = new Channel();
    channel.onmessage = (e) => {
      if (e.event === 'progress') {
        bar.value = e.done;
        status.textContent = `${Math.round(e.done * 100)}%`;
      } else {
        setRunning(false);
        status.textContent =
          e.event === 'finished' ? t('Saved as {path}', { path: e.path }) : e.error?.kind === 'cancelled' ? t('Stopped.') : t("Couldn't convert: {error}", { error: e.error?.what ?? e.error?.message ?? e.error?.kind });
      }
    };
    const options = { audioOnly: kind.value === 'audio', maxHeight: height.value ? Number(height.value) : null, videoKbps: Number(quality.value), audioKbps: 160 };
    setRunning(true);
    status.textContent = '';
    try {
      const started = await call('video_convert', { path: file.path, options, channel });
      if (!started) setRunning(false);
    } catch (e) {
      setRunning(false);
      status.textContent = t("Couldn't convert: {error}", { error: e?.what ?? e?.message ?? e?.kind ?? String(e) });
    }
  });
}
