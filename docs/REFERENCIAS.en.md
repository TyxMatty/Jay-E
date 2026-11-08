# Jay-E — Study of LibreOffice, ONLYOFFICE, MarkText, PhotoCraft, Kdenlive and OpenShot

[Português](REFERENCIAS.md) · **English**

> Translation of [REFERENCIAS.md](REFERENCIAS.md). The Portuguese file is the reference and is updated first, so this one may lag behind it.

Notes from reading the code of five open-source projects, done at the user's request before writing Jay-E. Jay-E is **MIT** (D1). From LibreOffice (MPL-2.0/LGPL), ONLYOFFICE (AGPL-3.0) and Kdenlive (GPL-3.0) come ideas, organization and numbers, **never code**. MarkText (MIT) and PhotoCraft (MIT/Apache-2.0) have compatible licenses; even so, only ideas for now, and a piece copied from either carries its license notice with it. The paths quoted are in their repositories.

Read on 2026-11-06: LibreOffice `core` `72b0b19` (2026-11-06), ONLYOFFICE `core` `ed63e94` and `sdkjs` `9d21388` (2026-10-15), MarkText `a520bef` (2026-11-06), PhotoCraft `b49c6c9` (2026-11-06), Kdenlive `4a4d449` (2026-11-06). Shallow, partial clones: of LibreOffice and ONLYOFFICE, only the READMEs, headers and file lists quoted.

## 1. LibreOffice (MPL-2.0, parts LGPL-3.0)

The whole suite (Writer, Calc, Impress, Draw), in C++, with UNO as its component model. More than 150 top-level modules.

### 1.1 Type and filter are separate things

- `filter/README.md`: the filter configuration has two parts, the **types** (`filter/source/config/fragments/types/`) and the **filters** (`.../filters/`). Each file type has **exactly one** type definition, and several filters can use it (e.g. plain text opens in Writer and in Calc).
- Detection looks at the content, not only the extension.
- **In Jay-E:** `jay_docs::detect` decides the type from the first bytes (the extension only breaks ties between identical containers, such as ZIP → DOCX or ODT); the viewer comes after, chosen by type. Done in stage 0.

### 1.2 Reader = tokenizer + mapper + model

- `sw/README.writerfilter.md`: DOCX and RTF import has **tokenizers** per format (`ooxml/`, `rtftok/`) that feed a common **domain mapper** (`dmapper/`), which writes into Writer's model. The mapper hides the document API from the tokenizer.
- For debugging, the traffic into the `dmapper` can be dumped to XML (`SW_DEBUG_WRITERFILTER=1`).
- **In Jay-E:** each reader (DOCX, ODT, RTF, Markdown) only translates the format's events to a common mapper that builds the internal model (section 2.1, the same idea). A JSON dump of the model serves as a regression test.

### 1.3 LibreOfficeKit: tiled rendering

- `libreofficekit/README.md` and `include/LibreOfficeKit/LibreOfficeKit.hxx`: a C API without UNO to convert documents (`documentLoad`, `saveAs(url, format, options)`) and, as an "unstable" API, **tiled rendering**: `paintTile(buffer, canvasWidth, canvasHeight, x, y, tileWidth, tileHeight)`, with the position in document coordinates (twips) and the buffer in 32-bit BGRA, rows top to bottom. `getDocumentSize`, `getParts`/`setPart` (pages or sheets).
- Tiled editing is a two-way channel: the client sends keys and clicks (`postKeyEvent`, `postMouseEvent`), and LibreOffice reports through a callback which rectangles it invalidated.
- It's what Collabora Online uses in the browser.
- **In Jay-E:** the PDF viewer (and later the large-image one) asks Rust for **tiles** by coordinate and zoom, and only redraws the invalidated rectangles. Rust hands over the finished tile through its own protocol (section 5 of the plan), without passing pixels through the JSON IPC.

### 1.4 PDF

- LibreOffice uses **PDFium** to read and draw PDF (`include/vcl/filter/PDFiumLibrary.hxx`, a C++ wrapper of it), and has its own PDF writer (`vcl/inc/pdf/pdfwriter_impl.hxx`, with R6 encryption, XMP, embedded fonts).
- **Importing a PDF as a drawing** (`sdext/README.md`) runs poppler in an **external process**: a malicious PDF that crashes the parser doesn't crash the editor.
- **In Jay-E:** PDFium was measured against hayro (pure Rust) in spike S1, and hayro won (`ARQUITETURA.en.md`, section 8). With it, a malicious PDF doesn't corrupt memory, so the parser stays in the same process, in a thread per document (D5).

### 1.5 Other points

- Conversion without a window: `soffice --headless --convert-to pdf file.docx`. A reference for checking Jay-E's output in tests, not a dependency (it's ~300 MB).
- LibreOffice already has a `rust_uno/` (a Rust binding of UNO, MPL-2.0), a sign the project is accepting Rust, but it depends on the whole installation.

## 2. ONLYOFFICE (AGPL-3.0)

Document, spreadsheet and presentation editors. The server (`core`, in C++) converts; the editor (`sdkjs`, in JavaScript) draws everything on a `<canvas>`.

### 2.1 Star-shaped conversion: one format in the middle

- `X2tConverter/src/ASCConverters.h` has the routes: `docx2doct`, `doct2docx`, `doct2rtf`, `doc2doct`, `html2doct`, `odf2oox`, `bin2pdf`, `md2html`, `hwp2docx`, `epub2docx_dir`, `fb2docx`… Almost everything goes through an **internal format** (`doct`, the `Editor.bin`) or through DOCX: each new format needs one reader and one writer, not a converter to every other format (2N instead of N²).
- The same internal format is what the editor loads: opening and converting use the same path.
- **In Jay-E:** an internal document model (blocks: headings, paragraphs with formatted runs, lists, tables, images, code, formulas), with one reader and one writer per format. Opening to read, editing and converting all go through it. It's the plan's central decision (section 6).

### 2.2 PDF → DOCX by rebuilding the structure

- `DocxRenderer/src/logic/`: to turn PDF into DOCX, it gathers glyphs into `TextLine`, lines into `Paragraph`, rectangles into `Table` and `Shape` (`elements/`), with font and style managers (`managers/`).
- **In Jay-E:** PDF → DOCX/Markdown is left for after v1 (it's the hardest converter and the one that gets most wrong); if it comes, it follows this order: glyphs → lines → paragraphs → tables.

### 2.3 Canvas editor

- `sdkjs/word/` has its own layout and pagination engine; text is drawn on the canvas, without `contentEditable`. That way the page comes out the same in any browser, but it's a whole text engine (fonts, line breaking, bidi, hyphenation).
- `sdkjs/pdf/src/viewer.js` (5,900 lines): a PDF viewer with the engine compiled to WebAssembly (`pdf/src/engine/drawingfile.wasm`), a **page cache** (`CCacheManager`), only the visible pages requested and drawn (`startVisiblePage`/`endVisiblePage`), thumbnails repainted one by one, zoom modes (width, page, free).
- **In Jay-E:** the PDF viewer copies this organization (visible pages, a bounded cache, thumbnails on demand, zoom modes), with the engine in Rust instead of wasm. Document editing does **not** reimplement a layout engine: it uses WebView2's HTML in pages, which already does fonts, bidi and line breaking (section 6). It's less faithful to Word than ONLYOFFICE, and that's accepted (risk 3).

## 3. MarkText (MIT)

A WYSIWYG Markdown editor in Electron + Vue 3. Its editing engine is **Muya** (`packages/muya`, TypeScript).

### 3.1 The engine

- Markdown becomes a **JSON state** (blocks) and back to Markdown (`state/markdownToState.ts`, `state/stateToMarkdown.ts`); HTML comes from the state (`markdownToHtml.ts`, `renderToStaticHTML.ts`).
- It draws the blocks with **snabbdom** (virtual DOM) and only redoes the block that changed; the rest of the page stays.
- **Undo** with **ot-json1** operations on the state (`history/index.ts`): a stack of at most 100, merging typing within 1 s, keeping the selection with it; operations on the whole document ask for a full rebuild.
- Conformance tested against the **CommonMark 0.31 and GFM** specs (`test/spec/`).
- Math with **KaTeX**, diagrams with Mermaid/PlantUML/Vega, embedded HTML cleaned by **DOMPurify**, with security tests (`e2e/tests/security`).

### 3.2 Files

- `main/filesystem/encoding.ts`: detects the encoding (UTF-8, BOM, and `ced` when it isn't UTF-8) and **saves in the same encoding**, so it doesn't swap bytes for `U+FFFD`. Latin-1 is treated as CP1252.
- `main/filesystem/watcher.ts`: watches the file and the folder (chokidar) and warns when another program changes the open file.
- `main/filesystem/documentWrites.ts`: a document's writes go into a queue; "read only" and IO barriers block new writes until the earlier ones finish.
- Exporting PDF is Chromium's `printToPDF` over the HTML.
- Focus, typewriter and source-code modes.

### 3.3 For Jay-E

- **Markdown → HTML in Rust** (`pulldown-cmark`, which also passes the CommonMark tests) and cleaning in Rust (`ammonia`), instead of `marked` + DOMPurify in JavaScript.
- **Editor:** first source code with a preview beside it (stage 2), block WYSIWYG later (open decision D4). Muya's WYSIWYG is the best model for that, and it's MIT.
- **Saving safely:** the same encoding, the same line endings, an atomic write (a `.tmp` file and a rename), watching for outside changes.
- **PDF through WebView2 itself**, like `printToPDF`: Markdown, DOCX and HTML become PDF without a new library.

## 4. PhotoCraft (MIT or Apache-2.0)

An image editor "in pure Rust", a clean-room reimplementation of Photoshop (only public specifications and observed behavior). 24 crates, an egui interface, a wgpu compositor.

### 4.1 Organization

- **Engine first** (`README.md`, "Under the hood"): a data-only document model (`crates/doc`) and a **command engine** (`crates/engine`); the interface is a thin layer on top. Every action is a command, callable from the UI, the CLI (`apps/photocraft-cli`), a JSON channel and an MCP server.
- **Layers checked at build time** (`AGENTS.md`, `cargo xtask layers`): each crate only depends on the ones below; `codecs`, `psd` and `cms` depend on nothing in the workspace; nothing below the UI uses egui.
- **Two compositors**: one on the CPU, which is the reference, and one on the GPU, tested against each other.
- **256² copy-on-write tiles**: cheap undo and huge images stay light.

### 4.2 Codecs

- Decoders in Rust: `image` (GIF, BMP, TGA, ICO, QOI, HDR), `png`, `zune-jpeg`, `jpeg-encoder`, `image-webp`, `tiff`, `exr`, lossless JPEG XL (`zune-jpegxl`); HEIF apart, optional.
- **A file is untrusted input**, with explicit limits in each format (`crates/codecs/src/codecs/`): `MAX_GIF_FRAMES = 100 000`, `MAX_IDAT = 1 GiB` in PNG, `MAX_IFDS = 10 000` and sub-IFD depth 4 in TIFF, up to 2,048 pixels per compressed byte in JPEG (decompression bomb), bounded ICC in ICO.
- Recursion always bounded (`AGENTS.md`: documents can be deep or cyclic).

### 4.3 For Jay-E

- `jay-docs` without Tauri, with the thin interface on top, like Jay-A's `jay-audio`: the same idea as the command engine. The editing operations (crop, rotate, convert) are data commands, testable without a window.
- The same Rust codecs for images (stage 3), with per-format limits and a total pixel limit.
- Tiles for large images, sharing the path of PDF tiles.
- What does **not** come in: layers, masks, brushes and PSD. Jay-E edits images at the "adjust and convert" level (crop, rotate, resize, brightness/contrast, mark up), not painting.

## 5. Kdenlive (GPL-3.0)

KDE's video editor, in C++/Qt. The heavy work is done by **MLT** (decoding, applying effects, rendering), which uses FFmpeg, frei0r, LADSPA, SoX.

### 5.1 Organization

- `dev-docs/architecture.md`: Kdenlive is the **model and the interface**; MLT does the video.
- **Non-destructive project** (`dev-docs/fileformat.md`): the `.kdenlive` is an MLT XML that only **references** the media; MLT renders the project file directly, ignoring Kdenlive's extra data. There was once a format with duplicated data that got out of sync (generation 1): the lesson is to keep each piece of data in one place.
- Timeline (`src/timeline2/model/`): tracks, clips, compositions, groups and snapping as separate models.
- **Functional undo** (`src/undohelper.hpp`): each operation builds an undo/redo pair of functions (`Fun`), chained with `PUSH_LAMBDA`; a compound operation is the sum of the small ones.
- **Background jobs** (`src/jobs/`): loading a clip, thumbnails, audio levels, proxy, transcoding, cutting, stabilizing, scene detection; a `taskmanager` with a queue.
- **Proxy**: a light copy of the clip to edit smoothly, swapped for the original when rendering (`kdenlivesettings.kcfg`: `enableproxy`, `generateproxy`).
- **Rendering in another process** (`renderer/kdenlive_render.cpp`): exporting a long video doesn't freeze the interface, and a failure doesn't bring down the editor.

### 5.2 For Jay-E

- Jay-E is **not** a video editor with tracks. For video: watch, **cut without re-encoding** (stream copy, instant), join pieces, extract a frame as an image, convert. A single track.
- From Kdenlive come: non-destructive editing (a list of cuts, the original doesn't change), jobs in a queue with progress and cancel, conversion in another process, thumbnails and waveform computed in the background, and undo as pairs of functions.
- The video engine is the weightiest decision (D3): FFmpeg is 80–130 MB; **Media Foundation** already comes with Windows (H.264, HEVC with an extension, AAC, MP4) and weighs nothing in the installer.

## 6. Tools for stage 10: PhotoCraft and OpenShot (2026-11-07)

Read at the user's request to choose the tools of stage 10 (first meant for stage 9, which became only the release): PhotoCraft's README (`storytold/photocraft`, MIT or Apache-2.0) and OpenShot's README and features page (`OpenShot/openshot-qt`, **GPL-3.0**). From OpenShot come only ideas, never code; from PhotoCraft, a copied piece carries its license notice with it (section 4).

### 6.1 PhotoCraft: what it has

A photo editor in pure Rust (egui on top of a document model and a command engine, 24 crates), alpha, that reimplements the Photoshop set: 68 tools (selection, crop and perspective crop, retouching with remove, healing brush, red eye, painting, local blur/sharpen/dodge, vectors, shapes, text), 16 adjustments that are also layers (per-channel curves, levels with a histogram, black and white, channel mixer, gradient map, photo filter, selective color, `.cube`/`.3dl`/`.look` LUT, vibrance, hue/saturation, shadows/highlights, automatic tone, contrast and color), filters with a live preview (blurs, sharpen, reduce noise, distortions, stylize), layers, masks and styles, ICC color management, PSD round trip, and an "Export as" with a preview and the file size before saving. Design: sparse 256×256 copy-on-write tiles (cheap undo), a GPU compositor (wgpu) checked against a CPU one, multithreaded filters (large blurs by running sums), 500+ commands in a single registry (interface, command line, JSON). It says itself that AI and typography are still weak.

### 6.2 OpenShot: what it has

A video editor in Python/Qt on top of libopenshot (C++) and FFmpeg: unlimited tracks, a timeline with thumbnails and waveform, trim, split (razor), resize, snapping, zoom and frame by frame; keyframe animation on curves (position, rotation, scale, opacity, volume, speed); speed and reverse; 400+ transitions (overlapping two clips makes one); effects (brightness, contrast, gamma, hue, black and white, chroma key, crop, blur, point masks); color correction with wheels, curves, LUT and scopes (waveform, histogram, vectorscope); SVG and 3D titles (through Blender), rolling credits; audio mixing and sound visualization; screen and webcam recording; export through any FFmpeg codec; EDL/XML. Hardware decoding and encoding is experimental.

### 6.3 For Jay-E (stage 10)

- **Images:** everything goes into stage 3's non-destructive edit list (undo, the original intact, the same list applied when batch converting). Curves and levels with a histogram; exposure, shadows/highlights, temperature and tint, vibrance; black and white; the three automatic ones; sharpen, blur, reduce noise, vignette; `.cube` LUT; straighten with a grid, crop to a ratio, perspective crop; mark up (text, arrows, shapes, pen, highlighter, blur or pixelate an area to hide data), red eye and remove a spot (clone); "Export" with a preview and the file size. Filters multithreaded with rayon, as in PhotoCraft; the markup stays editable until export and comes out drawn into the file.
- **Video:** a single track, no keyframes. A timeline with thumbnails, waveform and zoom, frame by frame; razor (several pieces, drop the middle ones, reorder), copied without re-encoding when the cuts fall on key frames, as in stage 5; speed (0.25× to 4×) and reverse; crop the frame, rotate 90°, flip; sound (volume, mute, fade in and out, save the sound alone, replace it or add music); fade to and from black and a dissolve between pieces; text and a watermark on top, from one time to another; brightness, contrast, saturation; blur an area; presets (WhatsApp, YouTube 1080p, GIF).
- **The video engine:** whatever changes the frames re-encodes through Media Foundation (D3), without FFmpeg (the installer stays small): the `SourceReader` decodes to RGB32 and PCM, Rust applies the edits frame by frame (on several threads), and the `SinkWriter` encodes H.264 + AAC, on the GPU when there is one. The GIF comes out of Rust (reduced frames and a palette).
- **Not adopted:** PhotoCraft's layers, masks, brushes with dynamics, vectors and PSD; OpenShot's multiple tracks, keyframes, the 400 transitions, 3D titles, chroma key, masks, scopes, screen recording and EDL.

## 7. What Jay-E adopts

| Idea | From | Where it goes in Jay-E |
|---|---|---|
| Type detected from the content, separate from the reader | LibreOffice | `jay_docs::detect` (stage 0 ✅) |
| One internal model in the middle: one reader and one writer per format | ONLYOFFICE (`doct`) | the document model (stage 4) and conversion (stage 8) |
| Reader = the format's tokenizer + a common mapper | LibreOffice (`writerfilter`) | the DOCX, ODT, RTF and Markdown readers |
| Tiles requested by coordinate and zoom, only what's visible, a bounded cache | LibreOfficeKit, ONLYOFFICE (`viewer.js`), PhotoCraft | the PDF viewer (stage 1) and large images (stage 3) |
| PDF drawn in Rust; the parser isolated from the rest | LibreOffice (PDFium, `pdfimport` in another process) | stage 1 ✅, with hayro (safe Rust) in a thread per document instead of another process |
| Markdown → state → Markdown, CommonMark/GFM conformance, clean HTML | MarkText | stage 2, with `pulldown-cmark` and `ammonia` in Rust |
| Save in the same encoding and line endings, atomic write, watch outside changes | MarkText | every text editor (stage 2) |
| PDF through the browser's printing | MarkText (`printToPDF`) | exporting Markdown, DOCX and HTML to PDF |
| An engine without an interface, actions as testable commands | PhotoCraft | the `jay-docs` crate |
| A file is untrusted input, per-format limits, bounded recursion | PhotoCraft | every reader |
| Image codecs in Rust | PhotoCraft | stage 3 |
| Non-destructive editing: a list of edits, the original doesn't change | Kdenlive | images (stage 3) and video (stage 5) |
| Undo as pairs of functions; undo with the selection along | Kdenlive, MarkText | every editor |
| Jobs in a queue with progress and cancel; conversion in another process | Kdenlive | conversion (stages 5 and 8) |

**Not adopted:** LibreOffice's UNO and whole suite (spreadsheets and presentations stay out of v1, D2), ONLYOFFICE's canvas layout engine, collaborative editing, PhotoCraft's layers and brushes, Kdenlive's and OpenShot's multiple tracks, and MarkText's Electron (Jay-E is Tauri, like the line). Image adjustments and simple video effects come in stage 10 (section 6).
