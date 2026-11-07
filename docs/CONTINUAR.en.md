# Continuing Jay-E on another PC

[Português](CONTINUAR.md) · **English**

Updated on 2026-11-07, after stage 6 (LaTeX). Everything needed to set Jay-E up from scratch, run it, understand how each part works and know what comes next. The full plan, with each stage's measurements, is in [`ARQUITETURA.md`](ARQUITETURA.md) (in Portuguese).

## 0. On a PC that already has the project

```powershell
git pull
npm install
npm test
```

If the pull brings LaTeX changes and vcpkg doesn't have its libraries yet, run the vcpkg step from section 1. Then `npm run dev`.

## 1. Install the tools (Windows)

In PowerShell. Each `winget` may ask for administrator approval (UAC).

```powershell
winget install -e --id Git.Git
winget install -e --id OpenJS.NodeJS.LTS
winget install -e --id Microsoft.VisualStudio.2022.BuildTools --override "--wait --passive --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
winget install -e --id Rustlang.Rustup
```

Then, **in a new terminal**, **vcpkg** with the LaTeX engine's C libraries (ICU, FreeType, Graphite2, fontconfig and libpng, all static). The first time takes about 30 minutes; ICU is most of it.

```powershell
git clone https://github.com/microsoft/vcpkg C:\Users\<you>\vcpkg
C:\Users\<you>\vcpkg\bootstrap-vcpkg.bat -disableMetrics
C:\Users\<you>\vcpkg\vcpkg install --triplet x64-windows-static-md icu freetype graphite2 fontconfig libpng
```

WebView2 already comes with Windows 11. No Python, LibreOffice, FFmpeg or installed TeX: Jay-E uses none of them.

## 2. Get the code

Clone **outside OneDrive**. The Rust build makes several GB and OneDrive would try to sync them.

```powershell
git clone https://github.com/TyxMatty/Jay-E.git C:\dev\Jay-E
cd C:\dev\Jay-E
npm install
```

Create `.cargo\config.toml` at the repository's root (it's local and stays out of git). `[env]` tells the build where vcpkg's libraries are; `[build]` is only needed if the clone is inside OneDrive:

```toml
[build]
target-dir = "C:/Users/<you>/.cargo-target/jay-e"

[env]
TECTONIC_DEP_BACKEND = "vcpkg"
VCPKG_ROOT = "C:/Users/<you>/vcpkg"
VCPKGRS_TRIPLET = "x64-windows-static-md"
```

## 3. Run

```powershell
npm run dev        # opens the app (the 1st build compiles the TeX engine; about 10 minutes)
npm run web        # only the interface in a browser, without Tauri (port 5177; opens no files)
npm run build      # the NSIS installer, in <target-dir>\release\bundle\nsis\
```

Tests:

```powershell
npm test
cd crates/jay-docs; cargo test
cd frontend/src-tauri; cargo test
```

**Driving the real app:** start it with remote debugging on; then a `.js` file runs inside its page, with the Rust commands working:

```powershell
$env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = '--remote-debugging-port=9226'; npm run dev
node scripts/cdp-eval.mjs test.js
node scripts/screenshots.mjs artigo.tex notas.md   # retakes the README's screenshots
```

`jay-e.exe <file>…` opens files; with Jay-E already open, they go to its window (single instance).

## 4. How Jay-E works

Two layers: the **engine** in Rust ([`crates/jay-docs`](../crates/jay-docs), no Tauri), which reads, draws, edits and converts; and the **app** ([`frontend`](../frontend)): the Tauri window (`src-tauri`, Rust) and the vanilla JS page (`src`), which only shows things and sends commands.

| Part | Where | How it works |
|---|---|---|
| Opening files | `detect.rs`, `src-tauri/src/files.rs` | The type is decided by the first 4 KB, not the extension. Each opened file (from the dialog, a drop, the command line or a second launch) goes into a list in Rust; the page never sends a path that isn't on it. |
| PDF | `pdf.rs`, `src-tauri/src/pdf.rs` | hayro (pure Rust) draws on one thread per document. Pages reach the page as PNG through the `jaye://` protocol, only the visible ones. Search, text and links also come from Rust. |
| Markdown and text | `text.rs`, `markdown.rs` | Saves in the file's own encoding and line endings, atomically, and warns if it changed on disk. The preview is made in Rust (pulldown-cmark, math as MathML) and cleaned by ammonia, so `<script>` in Markdown doesn't run. |
| LaTeX | `latex.rs`, `src-tauri/src/latex.rs` | Tectonic's engine (XeTeX) is built into `jay-e.exe`. To compile, the app runs itself with `--jaye-tex` in a separate process, with no window, in untrusted mode, with a time limit and Stop. Only the TeX packages come from the internet, the first time (42 MB for an article), and are cached. Errors become a list that jumps to the line. |
| Images | `image.rs` | Decoded in Rust, with limits against decompression bombs. Edits are a list applied over the original (so they can be undone); conversion uses libwebp for WebP and ravif for AVIF. |
| DOCX, ODT, RTF | `doc/` | Jay-E's own document model in the middle: each format is read into it and written from it (DOCX, Markdown, HTML, text). The page shows it on an A4 sheet; PDF comes from WebView2's printing. |
| Video | `video/mp4.rs`, `video/mf.rs` | Cutting and joining copy the MP4's samples without re-encoding (`mp4-atom` rewrites the tables). Converting and saving frames go through Windows' Media Foundation, which uses the GPU when there is one. |
| Updates | `src-tauri/src/update.rs` | At startup, checks the latest release on GitHub; the installer only runs if its SHA-256 matches GitHub's, as in Jay-H. |
| Styles and language | `frontend/src/styles`, `locales` | Jay-C's Calm, Neon and Soft styles, light or dark; English or Portuguese. |

## 5. Current state

| Stage | What it delivers |
|---|---|
| 0. Base | Window, styles, theme, language, single instance; four ways to open; type detection; one tab per file |
| 1. PDF | A reader drawn in Rust: only the visible pages, zoom, thumbnails, go to page, search and copy text, links, password. A 1,000-page PDF scrolls at 72 fps |
| 2. Markdown | Editor with a live preview and math; safe saving; export to HTML and PDF |
| 3. Images | Crop, rotate, flip, adjustments, undo; batch conversion; 100 MP photos |
| 4. Documents | DOCX, ODT, RTF, HTML and Markdown on an A4 sheet; export to DOCX, Markdown, HTML, text and PDF |
| 5. Video | Cut and join without re-encoding (10 s out of a 1 GB file in 42 ms); convert to MP4 or M4A; save frames |
| Updates | Checks GitHub at startup and updates itself; CI builds the installer on a version tag |
| 6. LaTeX | Compiles with Tectonic's engine inside the app: 1.4 s for an article with a bibliography (98 s the first time) |

**Size:** the installer is 16.5 MiB, above the 15 MB goal because of the TeX engine (ICU's data is most of it).

**Next stages:**
- **7. Editing:** DOCX and ODT with a lossless round trip; PDF: merge, split, reorder, rotate and delete pages, annotations and forms.
- **8. Converter:** one batch screen for every kind of file, with a queue, progress, cancel and asking before overwriting.
- **9. Release:** file associations ("Open with"), Jay-E in Jay-H's catalog, installing on a clean machine.

**To do** (the full list is in [`ROADMAP.md`](../ROADMAP.md)):
- publish the first release (v0.1.0), so the updater and Jay-H have something to find;
- LaTeX: compiling offline after the first time, "Save PDF…", Stop and the time limit in the app;
- see if ICU's data can be cut to what XeTeX uses, to bring the installer back to 15 MB;
- CI that runs `npm test` and both `cargo test` on every push.
