// Subtitles (.srt and .vtt): read into cues the video editor shows in the player (as WebVTT) and
// can burn into the export (each cue drawn as text, ui/video-editor.js).

/** "00:01:02,500", "01:02.5" or "1:02:03.250" → seconds; NaN if it isn't a time. */
function seconds(text) {
  const m = /^(?:(\d+):)?(\d{1,2}):(\d{1,2})(?:[.,](\d{1,3}))?$/.exec(text.trim());
  if (!m) return NaN;
  const [, h = '0', min, s, ms = '0'] = m;
  return Number(h) * 3600 + Number(min) * 60 + Number(s) + Number(ms.padEnd(3, '0')) / 1000;
}

/** The cues of SRT or WebVTT text: [{ from, to, text }] in seconds, in order. */
export function parseSubtitles(source) {
  const cues = [];
  for (const block of source.replace(/\r\n?/g, '\n').split(/\n{2,}/)) {
    const lines = block.split('\n').filter((l) => l.trim() !== '');
    const at = lines.findIndex((l) => l.includes('-->'));
    if (at < 0) continue;
    const [a, b] = lines[at].split('-->');
    const from = seconds(a);
    const to = seconds(b.trim().split(/\s+/)[0]);
    // Formatting tags (<i>, {\an8}) aren't drawn: only the words.
    const text = lines
      .slice(at + 1)
      .join('\n')
      .replace(/<[^>]+>|\{[^}]*\}/g, '')
      .trim();
    if (Number.isFinite(from) && Number.isFinite(to) && to > from && text) cues.push({ from, to, text });
  }
  return cues.sort((x, y) => x.from - y.from);
}

const stamp = (t) => {
  const ms = Math.round(t * 1000);
  const p = (n, w = 2) => String(n).padStart(w, '0');
  return `${p(Math.floor(ms / 3_600_000))}:${p(Math.floor(ms / 60_000) % 60)}:${p(Math.floor(ms / 1000) % 60)}.${p(ms % 1000, 3)}`;
};

/** The cues as WebVTT, for a <track> in the player. */
export function toWebVtt(cues) {
  return `WEBVTT\n\n${cues.map((c) => `${stamp(c.from)} --> ${stamp(c.to)}\n${c.text}`).join('\n\n')}\n`;
}
