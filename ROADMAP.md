# Jay-E roadmap

Jay-E is the editor and reader of the Jay line: documents, images and video, with conversion between formats. Windows first.

Every stage ends with something testable. The full plan, with the "done when" checks and the measurements of each finished stage, is in [`docs/ARQUITETURA.md`](docs/ARQUITETURA.md) (in Portuguese), section 8.

## Done

- [x] **0. Scaffold:** Tauri v2 + Vite + vanilla JS, Jay-C's three styles (Calm, Neon, Soft), single instance; open from the dialog, a drop, the command line or a second launch; file type detection; one tab per file.
- [x] **1. PDF reader:** drawn in Rust with hayro, only the visible pages; zoom, thumbnails, go to page, search, copy text, links, password. A 1,000-page PDF scrolls at 72 fps.
- [x] **2. Markdown and text:** CodeMirror with a live preview rendered in Rust (math included, sanitized by ammonia); saving keeps the file's encoding and line endings, atomically; export to HTML and PDF.
- [x] **3. Images:** non-destructive edits (crop, rotate, flip, brightness/contrast/saturation) with undo; batch conversion to WebP, AVIF, JPEG, PNG and more; 100 MP photos; decompression bombs refused.
- [x] **4. Documents:** one document model with DOCX, ODT, RTF, HTML and Markdown readers; paged A4 view; export to DOCX, Markdown, HTML, text and PDF.
- [x] **5. Video:** cut and join MP4/MOV without re-encoding (10 s out of a 1 GB file in 42 ms), thumbnail strip, save frames as PNG, convert to MP4 or M4A through Windows' Media Foundation.
- [x] **Updates:** Jay-E checks its latest GitHub release at startup and offers "Update to x.y.z"; the installer is checked against GitHub's SHA-256, unsaved edits are asked about, and Jay-E reopens when it's done. Releases are built by CI on a version tag.
- [x] **6. LaTeX:** compiled to PDF by Tectonic's TeX engine built into `jay-e.exe` (run as a child process, no downloaded program), the PDF beside the editor in the stage 1 reader, errors that jump to their line. An article with a bibliography: 1.4 s (98 s the first time, fetching 42 MB of TeX packages).

## Next

- [ ] **7. Editing:** DOCX and ODT (text, styles, lists, simple tables, images) with a lossless round trip; PDF: merge, split, reorder, rotate and delete pages, annotations, forms.
  - [x] DOCX edited in place with a word-processor toolbar, on the file's own page, fonts and styles; saving rewrites only the changed paragraphs (Word opens the result without repairs).
  - [x] ODT the same way (same toolbar, pictures, tables, lists, header and footer); RTF stays read-only (export to DOCX).
  - [x] Pictures resized and moved freely anywhere on the page; inserting pictures (file or Ctrl+V) and tables; headers and footers; the page split into the file's pages; Ctrl+Z/Ctrl+Y for everything; pasting with formatting.
  - [x] PDF: reorder, rotate, delete, insert another PDF and save pages apart; highlight, draw, notes, text, erase; fill forms; undo and save.
  - [ ] Rows and columns in existing tables; adding a header where there's none; first-page headers; tables split across pages.
- [ ] **8. Converter:** one batch screen for every kind of file, with a queue, progress, cancel and asking before overwriting. A corrupted file fails on its own and the others still convert.
- [ ] **9. Release:** NSIS installer in `%LOCALAPPDATA%\Jay-E`, file associations ("Open with"), Jay-E in Jay-H's catalog. Install on a clean machine and open a PDF with a double click.

## To do (found along the way)

- [ ] Publish the first release (v0.1.0) so the updater and Jay-H have something to find, and try updating on a test Windows account.
- [ ] Measure the PDF reader's first page in a release build (the 150 ms goal passes in 3 of 5 PDFs in dev).
- [ ] Try a PDF with a user password, an external link from a PDF, and screen readers on the pages.
- [ ] The light theme in the PDF reader, the editors and the document view.
- [ ] Video: HEVC, a phone video recorded sideways (rotation), the conversion's progress and Stop in the app; an M4A made by Media Foundation opens as a video.
- [ ] Documents: a large DOCX (hundreds of pages); ODT and RTF saved by Word.
- [ ] Markdown: a 50 MB file in the editor; the close confirmation with unsaved edits, checked in the app.
- [ ] LaTeX: compiling offline after the first time, "Save PDF…", Stop and the time limit in the app, a document with images.
- [ ] The installer grew to 16.5 MiB with the TeX engine (goal: 15 MB): see if ICU's data can be cut to what XeTeX uses.
- [ ] CI that runs `npm test` and both `cargo test` on every push.

## Open questions

- Spreadsheets (XLSX/ODS/CSV) and presentations (PPTX/ODP): out of v1 for now ("not supported"); read-only after stage 8 would need a cell model.
- Readers in a separate process: only if a PDF ever hangs the interpreter.
- An English version of the architecture document.
