# Jay-E

**English** · [Português](README.pt-BR.md)

A light, open-source desktop app to read, edit and convert documents, images and videos, with the heavy lifting done in Rust.

<p align="center"><img src="docs/images/latex.png" alt="Jay-E with a LaTeX article open: the source on the left and the compiled PDF on the right, with a bibliography" width="820"></p>

- **Word, OpenDocument and PDF edited in place:** Word and OpenDocument files on their own pages, fonts and styles, with the usual toolbar; PDFs highlighted, drawn on, filled in and their pages reordered. OpenDocument edited the same way; RTF opens and exports.
- **Markdown and LaTeX with a preview beside them**, math included, and LaTeX errors that jump to their line.
- **Photos and videos**: adjust and convert in batches, cut and join video without re-encoding.
- **No office suite**, no accounts and no ads: one installer that updates itself.

<table>
  <tr>
    <td><img src="docs/images/markdown-calm.png" alt="A Markdown note with its live preview in the Calm style, purple"></td>
    <td><img src="docs/images/markdown-neon.png" alt="A Markdown note with its live preview in the Neon style, cyan"></td>
    <td><img src="docs/images/markdown-soft.png" alt="A Markdown note with its live preview in the Soft style, orange"></td>
  </tr>
  <tr align="center"><td>Calm</td><td>Neon</td><td>Soft</td></tr>
</table>

## Download

Install and update Jay-E from **[Jay-H](https://github.com/TyxMatty/Jay-H)**, the Jay line's hub. Or download `Jay-E_<version>_x64-setup.exe` from the **[latest release](https://github.com/TyxMatty/Jay-E/releases/latest)**: it updates an earlier version in place and keeps your settings. Jay-E also checks for a newer release when it opens and offers to update itself. Every version is listed under [releases](https://github.com/TyxMatty/Jay-E/releases).

## Code

| Folder | Contents |
|---|---|
| [`crates/jay-docs/`](crates/jay-docs/) | The engine (Rust, no Tauri): file detection, PDF (hayro), Markdown, DOCX/ODT/RTF, images, video (MP4 stream copy, Media Foundation) and LaTeX (Tectonic's engine) |
| [`frontend/`](frontend/) | Tauri v2 + vanilla JS app (Vite), with CodeMirror 6 for the editors |
| [`docs/ARQUITETURA.md`](docs/ARQUITETURA.md) | Architecture, flows, stages and what was measured (in Portuguese) |
| [`docs/REFERENCIAS.md`](docs/REFERENCIAS.md) | Notes from studying LibreOffice, ONLYOFFICE, MarkText, PhotoCraft and Kdenlive (ideas only, no code) |
| [`docs/CONTINUAR.en.md`](docs/CONTINUAR.en.md) | Setting up on a new PC, how each part works, current state and next steps |
| [`ROADMAP.md`](ROADMAP.md) | What's done, what's next and the to-do list |
| [`scripts/`](scripts/) | Development tools: running JS in the real app, the README's screenshots |

## Running the app

```bash
npm install
npm run dev
```

This opens the Jay-E window (Tauri) with hot reload. `jay-e.exe <file>…` opens files; with Jay-E already open, they go to its window. To open only the interface in a browser, without Tauri, use `npm run web` (port 5177; it opens no files).

The build also needs **[vcpkg](https://github.com/microsoft/vcpkg)** with the LaTeX engine's C libraries. The first install takes about 30 minutes (ICU is most of it):

```bash
vcpkg install --triplet x64-windows-static-md icu freetype graphite2 fontconfig libpng
```

The Rust build takes several GB. Create a `.cargo/config.toml` (not tracked by git) that keeps it out of OneDrive and says where vcpkg is:

```toml
[build]
target-dir = "C:/Users/<you>/.cargo-target/jay-e"

[env]
TECTONIC_DEP_BACKEND = "vcpkg"
VCPKG_ROOT = "C:/Users/<you>/vcpkg"
VCPKGRS_TRIPLET = "x64-windows-static-md"
```

## Releases

A version tag (`v*`) builds the installer in GitHub Actions ([`installer.yml`](.github/workflows/installer.yml)) as a draft release; publishing it delivers the update through Jay-H and Jay-E's own updater. Bump the version in `frontend/src-tauri/tauri.conf.json`, `frontend/src-tauri/Cargo.toml` and `frontend/package.json` first: the tag has to match it.

## Tests

```bash
npm test
cd crates/jay-docs && cargo test
cd frontend/src-tauri && cargo test
```

To drive the real app, start it with `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9226`; then `node scripts/cdp-eval.mjs <file.js>` runs that file inside its page, and `node scripts/screenshots.mjs <file.tex> <file.md>` retakes the README's screenshots.
