// Runs a JS file inside the running Jay-E app (its WebView2 page) via the DevTools Protocol,
// so tests can drive the real app, Rust commands included. Start the app with remote debugging first:
// WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9226 (README). Copied from Jay-A.
//
//   node scripts/cdp-eval.mjs <file.js> [port]
//
// The file's contents run as the body of an async function; its return value is printed as JSON.
import { readFileSync } from 'node:fs';

const [, , file, port = '9226'] = process.argv;
if (!file) {
  console.error('usage: node scripts/cdp-eval.mjs <file.js> [port]');
  process.exit(2);
}

const targets = await (await fetch(`http://127.0.0.1:${port}/json`)).json();
// Dev loads http://localhost:5177; a release build serves the page as http://tauri.localhost.
const page = targets.find((t) => t.type === 'page' && /^https?:\/\/([^/]+:5177|tauri\.localhost)\//.test(t.url));
if (!page) throw new Error(`no Jay-E page among: ${targets.map((t) => t.url).join(', ')}`);

const ws = new WebSocket(page.webSocketDebuggerUrl);
await new Promise((resolve, reject) => {
  ws.onopen = resolve;
  ws.onerror = reject;
});
const expression = `(async () => {\n${readFileSync(file, 'utf8')}\n})()`;
ws.send(JSON.stringify({ id: 1, method: 'Runtime.evaluate', params: { expression, awaitPromise: true, returnByValue: true, userGesture: true } }));
const reply = await new Promise((resolve) => {
  ws.onmessage = (event) => {
    const msg = JSON.parse(event.data);
    if (msg.id === 1) resolve(msg);
  };
});
ws.close();

const { result, exceptionDetails } = reply.result ?? {};
if (exceptionDetails) {
  console.error('EXCEPTION', exceptionDetails.exception?.description ?? exceptionDetails.text);
  process.exit(1);
}
console.log(JSON.stringify(result?.value, null, 2));
