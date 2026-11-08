import { test } from 'node:test';
import assert from 'node:assert/strict';
import { parseSubtitles, toWebVtt } from '../src/core/subtitles.js';

test('SRT and WebVTT cues', () => {
  const srt = '1\r\n00:00:01,000 --> 00:00:03,500\r\nOlá, <i>Jay-E</i>!\r\n\r\n2\r\n00:01:02,250 --> 00:01:04,000\r\n{\\an8}Segunda\r\nlinha dois\r\n\r\nbroken\r\n';
  assert.deepEqual(parseSubtitles(srt), [
    { from: 1, to: 3.5, text: 'Olá, Jay-E!' },
    { from: 62.25, to: 64, text: 'Segunda\nlinha dois' },
  ]);
  const vtt = 'WEBVTT\n\n00:05.5 --> 00:07.000 align:start\nCurta\n';
  assert.deepEqual(parseSubtitles(vtt), [{ from: 5.5, to: 7, text: 'Curta' }]);
  // Back and forth through WebVTT.
  assert.deepEqual(parseSubtitles(toWebVtt(parseSubtitles(srt))), parseSubtitles(srt));
  assert.deepEqual(parseSubtitles('00:00:05,000 --> 00:00:04,000\nbackward'), []);
});
