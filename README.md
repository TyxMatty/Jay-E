# Jay-E

**English** · [Português](README.pt-BR.md)

The editor and reader of the Jay line: documents (PDF, DOCX, ODT, RTF, Markdown, LaTeX, text), images and videos, with conversion between formats. Lightweight, quick to render, Rust first. Windows first.

Plan and stages: [`docs/ARQUITETURA.md`](docs/ARQUITETURA.md); the study of LibreOffice, ONLYOFFICE, MarkText, PhotoCraft and Kdenlive behind it: [`docs/REFERENCIAS.md`](docs/REFERENCIAS.md) (both in Portuguese). For now (stage 0) the app has its window (in Jay-C's Calm, Neon and Soft styles) and opens files from the Open button (Ctrl+O), a drop on the window or the command line, one tab each. Images, video, audio and text show; PDF, DOCX, Markdown and LaTeX get their viewers in the next stages.

## Running

Node 22+ and Rust with MSVC (see Jay-C's `docs/CONTINUAR.en.md`).

```bash
npm install
npm run dev
```

`npm run dev` opens the Jay-E window (Tauri) with live reload. `jay-e.exe <file>…` opens files; with Jay-E already open, they go to its window. To open just the interface in a browser, without Tauri: `npm run web` (port 5177; it opens no files).

To keep the Rust build out of OneDrive, create a `.cargo/config.toml` (not tracked by git):

```toml
[build]
target-dir = "C:/Users/<you>/.cargo-target/jay-e"
```

## Tests

```bash
npm test
cd crates/jay-docs && cargo test
cd frontend/src-tauri && cargo test
```

To drive the real app: start it with `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9226`, then `node scripts/cdp-eval.mjs <file.js>` runs that file inside its page.

## License

MIT.
