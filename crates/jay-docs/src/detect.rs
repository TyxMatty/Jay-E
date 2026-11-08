//! What a file is, from its first bytes; the extension only decides between formats that share a
//! container (a ZIP is DOCX or ODT, plain text is Markdown, LaTeX or text). Like LibreOffice's type
//! detection, this is separate from the readers: one type, then whichever viewer handles it.

use serde::Serialize;
use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

/// Bytes read from the start of a file: enough for every signature below and the ODT `mimetype`.
const HEAD: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Pdf,
    /// Word processing: DOCX, ODT, RTF.
    Document,
    Markdown,
    Latex,
    Text,
    Image,
    Video,
    Audio,
    Unknown,
}

/// The kind and a short format name ("png", "docx", "mp4"…), shown in the UI.
pub fn detect(path: &Path) -> io::Result<(Kind, &'static str)> {
    let mut head = Vec::with_capacity(HEAD);
    File::open(path)?.take(HEAD as u64).read_to_end(&mut head)?;
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    Ok(detect_bytes(&head, &ext))
}

/// `ext` is lowercase, without the dot.
pub fn detect_bytes(head: &[u8], ext: &str) -> (Kind, &'static str) {
    let at = |offset: usize, sig: &[u8]| head.get(offset..offset + sig.len()) == Some(sig);
    use Kind::*;

    // UTF-16 text has NULs and its BOM looks like an MP3 frame: decide it first.
    if at(0, b"\xff\xfe") || at(0, b"\xfe\xff") {
        return text_kind(ext);
    }
    if at(0, b"%PDF-") {
        return (Pdf, "pdf");
    }
    if at(0, b"{\\rtf") {
        return (Document, "rtf");
    }
    if at(0, b"PK\x03\x04") {
        // ODF requires an uncompressed `mimetype` entry first (ODF 1.2, part 3, 3.3).
        if at(30, b"mimetype") && at(38, b"application/vnd.oasis.opendocument.text") {
            return (Document, "odt");
        }
        // DOCX has no fixed first entry: the extension decides among ZIPs.
        return match ext {
            "docx" | "docm" | "dotx" => (Document, "docx"),
            _ => (Unknown, "zip"),
        };
    }

    // Images.
    if at(0, b"\x89PNG\r\n\x1a\n") {
        return (Image, "png");
    }
    if at(0, b"\xff\xd8\xff") {
        return (Image, "jpeg");
    }
    if at(0, b"GIF87a") || at(0, b"GIF89a") {
        return (Image, "gif");
    }
    if at(0, b"RIFF") && at(8, b"WEBP") {
        return (Image, "webp");
    }
    // "BM" then four reserved zero bytes: a text file starting with "BM" has no NUL.
    if at(0, b"BM") && at(6, b"\0\0\0\0") {
        return (Image, "bmp");
    }
    // Cameras' raw files: most are TIFF inside (and would read as their small preview), others
    // have their own headers. By extension; the image editor reads them through Windows.
    if matches!(ext, "cr2" | "cr3" | "nef" | "nrw" | "arw" | "dng" | "orf" | "rw2" | "raf" | "srw" | "pef") {
        return (Image, "raw");
    }
    if at(0, b"II*\0") || at(0, b"MM\0*") {
        return (Image, "tiff");
    }
    if at(0, b"\0\0\x01\0") {
        return (Image, "ico");
    }
    if at(0, b"qoif") {
        return (Image, "qoi");
    }
    if at(0, b"<svg") || (at(0, b"<?xml") && ext == "svg") {
        return (Image, "svg");
    }

    // ISO base media (MP4, MOV, AVIF, HEIC): `ftyp` box at offset 4, then the major brand.
    if at(4, b"ftyp") {
        return match head.get(8..12).unwrap_or_default() {
            b"avif" | b"avis" => (Image, "avif"),
            b"heic" | b"heix" | b"mif1" | b"msf1" => (Image, "heic"),
            b"qt  " => (Video, "mov"),
            b"M4A " | b"M4B " => (Audio, "m4a"),
            _ => (Video, "mp4"),
        };
    }
    if at(0, b"\x1a\x45\xdf\xa3") {
        // EBML: the DocType says WebM or Matroska.
        let webm = head.windows(4).any(|w| w == b"webm");
        return (Video, if webm { "webm" } else { "mkv" });
    }
    if at(0, b"RIFF") && at(8, b"AVI ") {
        return (Video, "avi");
    }

    // Audio (played, not edited: editing audio is Jay-A's).
    if (at(0, b"ID3") && matches!(head.get(3), Some(2..=4)) && head.get(4) == Some(&0)) || (head.len() > 1 && head[0] == 0xff && head[1] & 0xe0 == 0xe0) {
        return (Audio, "mp3");
    }
    if at(0, b"RIFF") && at(8, b"WAVE") {
        return (Audio, "wav");
    }
    if at(0, b"fLaC") {
        return (Audio, "flac");
    }
    if at(0, b"OggS") {
        return (Audio, "ogg");
    }

    if looks_like_text(head) { text_kind(ext) } else { (Unknown, "binary") }
}

fn text_kind(ext: &str) -> (Kind, &'static str) {
    match ext {
        "md" | "markdown" | "mdown" | "mkd" => (Kind::Markdown, "markdown"),
        "tex" | "latex" | "ltx" | "sty" | "cls" | "bib" => (Kind::Latex, "latex"),
        _ => (Kind::Text, "text"),
    }
}

/// No NUL byte and almost no control characters. Not "valid UTF-8": legacy text (Windows-1252,
/// Latin-1) is text too, and text.rs works out its encoding.
fn looks_like_text(head: &[u8]) -> bool {
    if head.contains(&0) {
        return false;
    }
    let controls = head.iter().filter(|&&b| (b < 0x20 && !matches!(b, b'\t' | b'\n' | b'\r' | 0x0c)) || b == 0x7f).count();
    controls * 100 <= head.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use Kind::*;

    fn kind(head: &[u8], ext: &str) -> (Kind, &'static str) {
        detect_bytes(head, ext)
    }

    #[test]
    fn signatures_win_over_the_extension() {
        assert_eq!(kind(b"%PDF-1.7\n", "txt"), (Pdf, "pdf"));
        assert_eq!(kind(b"\x89PNG\r\n\x1a\n....", "jpg"), (Image, "png"));
        assert_eq!(kind(b"\xff\xd8\xff\xe0", "png"), (Image, "jpeg"));
        assert_eq!(kind(b"GIF89a", ""), (Image, "gif"));
        assert_eq!(kind(b"RIFF\0\0\0\0WEBPVP8 ", ""), (Image, "webp"));
        assert_eq!(kind(b"RIFF\0\0\0\0WAVEfmt ", ""), (Audio, "wav"));
        assert_eq!(kind(b"RIFF\0\0\0\0AVI LIST", ""), (Video, "avi"));
        assert_eq!(kind(b"{\\rtf1\\ansi", "doc"), (Document, "rtf"));
    }

    #[test]
    fn iso_media_by_brand() {
        assert_eq!(kind(b"\0\0\0\x20ftypisom\0\0\x02\0", ""), (Video, "mp4"));
        assert_eq!(kind(b"\0\0\0\x14ftypqt  \0\0\0\0", ""), (Video, "mov"));
        assert_eq!(kind(b"\0\0\0\x1cftypavif\0\0\0\0", ""), (Image, "avif"));
        assert_eq!(kind(b"\0\0\0\x18ftypheic\0\0\0\0", ""), (Image, "heic"));
        assert_eq!(kind(b"II*\0\x10\0\0\0CR", "cr2"), (Image, "raw"));
        assert_eq!(kind(b"II*\0\x08\0\0\0", "tif"), (Image, "tiff"));
    }

    #[test]
    fn matroska_or_webm_by_doctype() {
        assert_eq!(kind(b"\x1a\x45\xdf\xa3\x9f\x42\x86\x81\x01\x42\x82\x84webm", ""), (Video, "webm"));
        assert_eq!(kind(b"\x1a\x45\xdf\xa3\x9f\x42\x86\x81\x01\x42\x82\x88matroska", ""), (Video, "mkv"));
    }

    #[test]
    fn zip_documents() {
        let mut odt = b"PK\x03\x04".to_vec();
        odt.resize(30, 0);
        odt.extend_from_slice(b"mimetypeapplication/vnd.oasis.opendocument.text");
        assert_eq!(kind(&odt, "zip"), (Document, "odt"));
        // An ODF spreadsheet is not a text document.
        let mut ods = odt[..38].to_vec();
        ods.extend_from_slice(b"application/vnd.oasis.opendocument.spreadsheet");
        assert_eq!(kind(&ods, "ods"), (Unknown, "zip"));
        assert_eq!(kind(b"PK\x03\x04\x14\0", "docx"), (Document, "docx"));
        assert_eq!(kind(b"PK\x03\x04\x14\0", "zip"), (Unknown, "zip"));
    }

    #[test]
    fn text_by_extension() {
        assert_eq!(kind(b"# Title\n", "md"), (Markdown, "markdown"));
        assert_eq!(kind(b"\\documentclass{article}", "tex"), (Latex, "latex"));
        assert_eq!(kind("olá, mundo".as_bytes(), "txt"), (Text, "text"));
        assert_eq!(kind(b"", "md"), (Markdown, "markdown"));
        // A UTF-8 character cut by the 4 KB window is still text.
        assert_eq!(kind(&"ã".as_bytes()[..1], "txt"), (Text, "text"));
        assert_eq!(kind(b"\xff\xfeh\0i\0", "txt"), (Text, "text"));
    }

    #[test]
    fn text_that_starts_like_a_signature_is_still_text() {
        assert_eq!(kind(b"BMW notes", "txt"), (Text, "text"));
        assert_eq!(kind(b"ID3 tags to fix", "md"), (Markdown, "markdown"));
        assert_eq!(kind(b"BM6\0\x0c\0\0\0\0\06\0", ""), (Image, "bmp"));
        assert_eq!(kind(b"ID3\x04\0\0\0\0\0", ""), (Audio, "mp3"));
    }

    #[test]
    fn binary_is_unknown() {
        assert_eq!(kind(b"abc\0def", "md"), (Unknown, "binary"));
        assert_eq!(kind(b"\x01\x02\x03 control \x04\x05 bytes", "txt"), (Unknown, "binary"));
        // Windows-1252 isn't UTF-8 but is text.
        assert_eq!(kind(b"T\xedtulo \x93aspas\x94\r\n", "md"), (Markdown, "markdown"));
    }

    #[test]
    fn reads_a_real_file() {
        let dir = std::env::temp_dir().join(format!("jay-docs-detect-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("note.MD");
        std::fs::write(&file, "# Hello\n").unwrap();
        assert_eq!(detect(&file).unwrap(), (Markdown, "markdown"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
