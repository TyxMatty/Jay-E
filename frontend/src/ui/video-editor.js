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

  const root = h('div', { class: 'video-editor' }, toolbar, h('div', { class: 'video-stage' }, video), razorBar, stripScroll);
  root.addEventListener('keydown', (e) => {
    if (e.ctrlKey || e.target.closest('input, select')) return;
    if (e.altKey && (e.key === 'ArrowLeft' || e.key === 'ArrowRight')) return e.preventDefault(), move(e.key === 'ArrowLeft' ? -1 : 1);
    if (e.altKey) return;
    if (e.key === 'i' || e.key === 'I') markStart();
    else if (e.key === 'o' || e.key === 'O') markEnd();
    else if (e.key === 's' || e.key === 'S') split();
    else if (e.key === 'Delete') toggleKeep();
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
