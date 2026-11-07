// Takes the README's screenshots from the running app (started with remote debugging on 9226, see
// README), as Jay-A does. Open the files to show first; this script picks the tabs by name: a .tex
// (its PDF beside the editor) and a .md. Saves PNGs to docs/images/.
//
//   node scripts/screenshots.mjs <file.tex> <file.md> [port]
import { writeFileSync, mkdirSync } from 'node:fs';

const [, , tex, md, port = '9226'] = process.argv;
if (!tex || !md) {
  console.error('usage: node scripts/screenshots.mjs <file.tex name> <file.md name> [port]');
  process.exit(2);
}
const targets = await (await fetch(`http://127.0.0.1:${port}/json`)).json();
const page = targets.find((t) => t.type === 'page' && /^https?:\/\/([^/]+:5177|tauri\.localhost)\//.test(t.url));
if (!page) throw new Error('no Jay-E page');
const ws = new WebSocket(page.webSocketDebuggerUrl);
await new Promise((resolve, reject) => ((ws.onopen = resolve), (ws.onerror = reject)));
let id = 0;
const pending = new Map();
ws.onmessage = (event) => {
  const msg = JSON.parse(event.data);
  pending.get(msg.id)?.(msg);
};
const send = (method, params = {}) =>
  new Promise((resolve) => {
    pending.set(++id, resolve);
    ws.send(JSON.stringify({ id, method, params }));
  });
const run = async (code) => {
  const reply = await send('Runtime.evaluate', { expression: `(async () => {${code}})()`, awaitPromise: true, returnByValue: true, userGesture: true });
  if (reply.result?.exceptionDetails) throw new Error(reply.result.exceptionDetails.exception?.description);
  return reply.result?.result?.value;
};
const shot = async (file, height) => {
  await send('Emulation.setDeviceMetricsOverride', { width: 1100, height, deviceScaleFactor: 1, mobile: false });
  await new Promise((r) => setTimeout(r, 1200));
  const { result } = await send('Page.captureScreenshot', { format: 'png' });
  writeFileSync(`docs/images/${file}`, Buffer.from(result.data, 'base64'));
  console.log('saved', file);
};
const wait = 'await new Promise((r) => setTimeout(r, 400));';
const tab = (...names) => `[...document.querySelectorAll('[role=tab]')].find((b) => ${JSON.stringify(names)}.includes(b.textContent.trim()))?.click(); ${wait}`;
const style = (value) => `${tab('Settings', 'Configurações')} document.querySelector('input[name=style][value=${value}]')?.click(); ${wait}`;

mkdirSync('docs/images', { recursive: true });
await run(style('calm'));
await run(tab('Files', 'Arquivos') + tab(tex));
await shot('latex.png', 720);
for (const s of ['calm', 'neon', 'soft']) {
  await run(style(s));
  await run(tab('Files', 'Arquivos') + tab(md));
  await shot(`markdown-${s}.png`, 620);
}
await run(style('calm'));
await send('Emulation.clearDeviceMetricsOverride');
ws.close();
