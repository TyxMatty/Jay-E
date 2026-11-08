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
  - [x] UI pass on the Jay brand (from Jay-C): the editing toolbars in Jay-C's segmented groups, in each style's own radius, the pressed controls in the accent; the window and taskbar icon is the wolf in the style's colors and changes with it, also in dev runs started from inside another app's package.
  - [x] Rows and columns in the file's tables (insert above/below/left/right, delete); a header or footer where the file had none; long tables split row by row across pages, as in Word.
  - [x] First-page and even-page headers and footers ("Different first page", "Different odd & even pages"); a table's header row repeated at the top of each page it goes on to.
- [x] **8. Converter:** one batch screen for every kind of file (images, DOCX/ODT/RTF/Markdown, PDF to text, video and audio), one format per kind, a queue with progress per file, Stop, and asking before overwriting (keep both, replace, skip). 20 mixed files in 3 s; the damaged one failed alone and the other 18 came out.
- [x] **9. Release:** [v0.1.0](https://github.com/TyxMatty/Jay-E/releases/tag/v0.1.0) published on 2026-11-07: NSIS installer in `%LOCALAPPDATA%\Jay-E` (17 MB), file associations ("Open with"), Jay-E in Jay-H's catalog. Installed, a PDF opened with a double click, and Jay-H found Jay-E.
- [ ] **10. More tools:** the image and video editors grow (ideas from PhotoCraft and OpenShot, `docs/REFERENCIAS.md` section 6), and more ideas for every kind of file.
  - [x] Images, tone and color: Curves and Levels with a histogram; exposure, highlights/shadows, temperature and tint, vibrance; black and white; Auto tone, contrast and color. All in the edit list, undoable.
  - [x] Jay brand: the welcome tour and the dialogs animate as in Jay-C (Settings › Appearance › Animations, On/Off), and the accent glow of every Jay app sits behind the sections.
  - [x] Images, filters: sharpen, blur, reduce noise, vignette; color looks from `.cube` LUT files.
  - [x] Images, geometry: straighten (free rotation with a grid), crop to a ratio (1:1, 4:3, 16:9…), perspective crop.
  - [x] Images, drawing: pen and highlighter in any color and size, over the picture; each stroke undoes on its own and follows later rotations, flips and crops.
  - [x] Images, marking up: text, arrows, rectangles and ellipses, blur or pixelate an area (hide data).
  - [x] Images: red eye; remove a spot.
  - [x] Images: Export with a preview and the file size before saving.
  - [x] Video, timeline: thumbnails and the audio waveform, zoom, frame by frame (, and .).
  - [x] Video, razor: split into several pieces, drop the ones in the middle, reorder them; copied without re-encoding.
  - [x] Video, speed (0.25× to 4×); a frame shape (1:1, 9:16, 16:9, 4:5), rotate 90°, flip.
  - [x] Video: play in reverse.
  - [x] Video, sound: volume, mute, fade in and out, save the sound alone.
  - [x] Video: replace the sound or add music.
  - [x] Video, fades to and from black; text and a watermark picture over the video, from a time to a time; brightness, contrast, saturation.
  - [x] Video: a dissolve between the razor's pieces; blur an area.
  - [x] Video: export presets (WhatsApp, YouTube 1080p, small file, sound only).
  - [x] Video: animated GIF.
  - [x] Images: the same edits on many pictures at once; read HEIC and RAW through Windows' own codecs (WIC); see the photo's details (place, camera) and save without them; before/after.
  - [x] Video: shrink to a size (25 MB for Discord, 16 MB for WhatsApp).
  - [x] Video: subtitles (`.srt`, `.vtt`) shown in the player and burned in; save a frame every N seconds.
  - [ ] PDF: compress (images at a lower resolution); a drawn signature; black out text for good (redaction, the text removed, not covered); make it searchable with Windows' own OCR (`Windows.Media.Ocr`); pictures into one PDF and pages as pictures.
  - [ ] Documents: find and replace, word count, spelling in the file's language; compare two versions of a DOCX.
  - [ ] Converter: read HTML; LaTeX to PDF; a folder watched for new files.

## To do (found along the way)

- [ ] Converter: try Stop in the middle of a batch in the app.

- [x] Publish the first release: [v0.1.0](https://github.com/TyxMatty/Jay-E/releases/tag/v0.1.0) on 2026-11-07 (17 MB installer, SHA-256 on GitHub).
- [ ] Try updating from one version to the next (needs a second release).
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
