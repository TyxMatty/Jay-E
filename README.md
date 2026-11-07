# Jay-E

**English** · [Português](README.pt-BR.md)

The editor and reader of the Jay line: documents (PDF, DOCX, ODT, RTF, Markdown, LaTeX, text), images and videos, with conversion between formats. Lightweight, quick to render, Rust first. Windows first.

Plan and stages: [`docs/ARQUITETURA.md`](docs/ARQUITETURA.md); the study of LibreOffice, ONLYOFFICE, MarkText, PhotoCraft and Kdenlive behind it: [`docs/REFERENCIAS.md`](docs/REFERENCIAS.md) (both in Portuguese). For now (stage 5) the app has its window (in Jay-C's Calm, Neon and Soft styles) and opens files from the Open button (Ctrl+O), a drop on the window or the command line, one tab each. PDFs open in Jay-E's own reader, drawn in Rust with hayro (thumbnails, zoom, search, copying text, links); Markdown, LaTeX and text open in an editor that saves in the file's own encoding and line endings, Markdown with a live preview rendered in Rust (math included) and export to HTML and PDF; images open in an editor (rotate, flip, crop, brightness/contrast/saturation, undo, zoom into 100 MP photos) and convert, one or all at once, to WebP, AVIF, JPEG, PNG and more; videos play with a thumbnail strip, cut and join without re-encoding (instant, even on a 1 GB file), convert to MP4 or M4A through Windows' Media Foundation and save frames as PNG; audio plays; Word (DOCX), OpenDocument (ODT) and RTF documents open on an A4 sheet and export to DOCX, Markdown, HTML, text and PDF.

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
