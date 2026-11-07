# Jay-E

**English** · [Português](README.pt-BR.md)

The editor and reader of the Jay line: documents (PDF, DOCX, ODT, RTF, Markdown, LaTeX, text), images and videos, with conversion between formats. Lightweight, quick to render, Rust first. Windows first.

It's for people who want one small, fast app to open, fix up and convert everyday files (a PDF to read, a DOCX to turn into Markdown, a batch of photos to shrink into WebP, a clip to cut out of a long video) without installing an office suite, a photo editor and a video editor. It's part of the Jay line with [Jay-C](https://github.com/TyxMatty/Jay-C) (calls), [Jay-A](https://github.com/TyxMatty/Jay-A) (audio) and [Jay-H](https://github.com/TyxMatty/Jay-H) (the hub that installs them).

## What it does today (stage 5)

- **Opening files:** the Open button (Ctrl+O), a drop on the window or the command line, one tab each.
- **PDF:** Jay-E's own reader, drawn in Rust with hayro: thumbnails, zoom, search, copying text, links, password-protected PDFs.
- **Markdown, LaTeX and text:** an editor that saves in the file's own encoding and line endings; Markdown with a live preview rendered in Rust (math included) and export to HTML and PDF.
- **Images:** rotate, flip, crop, brightness/contrast/saturation, undo, zoom into 100 MP photos; convert one or all at once to WebP, AVIF, JPEG, PNG and more.
- **Video and audio:** playback with a thumbnail strip; cut and join without re-encoding (instant, even on a 1 GB file); convert to MP4 or M4A through Windows' Media Foundation; save frames as PNG.
- **Word (DOCX), OpenDocument (ODT) and RTF:** shown on an A4 sheet, exported to DOCX, Markdown, HTML, text and PDF.
- **Updates:** Jay-E checks its latest GitHub release at startup and offers to update itself (see [Updates](#updates)).
- The window comes in Jay-C's Calm, Neon and Soft styles, light or dark, in English or Portuguese.

What's next, and the to-do list: [`ROADMAP.md`](ROADMAP.md). The full plan and stages: [`docs/ARQUITETURA.md`](docs/ARQUITETURA.md); the study of LibreOffice, ONLYOFFICE, MarkText, PhotoCraft and Kdenlive behind it: [`docs/REFERENCIAS.md`](docs/REFERENCIAS.md) (both in Portuguese).

## Built with

- **Rust** for everything that reads or writes a file (`crates/jay-docs`): [hayro](https://github.com/LaurenzV/hayro) for PDF, pulldown-cmark + ammonia for Markdown, the `image` crate, ravif and libwebp for images, and Windows' Media Foundation for video.
- **Tauri 2** for the window (`frontend/src-tauri`), with WebView2.
- **Vanilla JavaScript** + Vite for the interface (`frontend/src`), with CodeMirror 6 for the editors.

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

`npm run build` makes the Windows installer (NSIS) in `frontend/src-tauri/target/release/bundle/nsis/`.

## Tests

```bash
npm test
cd crates/jay-docs && cargo test
cd frontend/src-tauri && cargo test
```

To drive the real app: start it with `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9226`, then `node scripts/cdp-eval.mjs <file.js>` runs that file inside its page.

## Updates

At startup Jay-E asks GitHub for this repository's latest release. When it's newer than the running version, an "Update to x.y.z" button shows in the top bar. Clicking it downloads the release's `*_x64-setup.exe`, which only runs if its SHA-256 matches the one GitHub computed, asks about unsaved edits, then runs the installer in passive mode (settings kept) and reopens Jay-E. It's the same check Jay-H uses to install the Jay apps; offline, nothing shows.

To release a version:

1. Bump the version in `frontend/src-tauri/tauri.conf.json`, `frontend/src-tauri/Cargo.toml` and `frontend/package.json`.
2. Tag it and push the tag: `git tag v0.2.0 && git push origin v0.2.0`.
3. The **Installer** workflow builds the installer and creates a draft release with it. Write the notes, try the installer, then publish the release: from then on, every Jay-E finds it.

## License

[MIT](LICENSE).
