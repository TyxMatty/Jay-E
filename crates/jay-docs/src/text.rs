//! Text files that are saved back the way they came: same encoding, same byte-order mark, same line
//! endings, written atomically, and not over a file someone else changed meanwhile (MarkText's
//! `encoding.ts` and `documentWrites.ts`, docs/REFERENCIAS.md 3.2).
//!
//! Encodings: UTF-8 (with or without BOM), UTF-16 with a BOM, and Windows-1252 for anything that
//! isn't valid UTF-8. 1252 maps every byte to a char (the 5 undefined ones to C1 controls, as the
//! WHATWG does), so a file in another legacy encoding still round-trips byte for byte if untouched.

use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

/// Larger files aren't opened as text (the editor holds the whole file).
pub const MAX_TEXT_BYTES: u64 = 50 << 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Encoding {
    Utf8,
    Utf8Bom,
    Utf16Le,
    Utf16Be,
    Windows1252,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LineEnding {
    Lf,
    Crlf,
}

/// What the file was when read: saving checks it's still the same (size and modification time).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Stamp {
    pub len: u64,
    /// Microseconds since 1970: nanoseconds would pass 2^53 and lose precision as a JS number.
    pub modified_us: u64,
}

/// A text file: `text` always has `\n` line endings; the rest says how to write it back.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextFile {
    pub text: String,
    pub encoding: Encoding,
    pub line_ending: LineEnding,
    pub stamp: Stamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum TextError {
    TooBig,
    /// Not text (a NUL byte outside UTF-16).
    Binary,
    /// Changed on disk since it was read: saving would lose those changes.
    Changed,
    /// Has characters the file's encoding can't hold (only Windows-1252 can fail): the first one.
    Unencodable { ch: String },
    Io { message: String },
}

impl From<std::io::Error> for TextError {
    fn from(e: std::io::Error) -> Self {
        Self::Io { message: e.to_string() }
    }
}

pub fn stamp(path: &Path) -> Result<Stamp, TextError> {
    let meta = fs::metadata(path)?;
    let modified_us = meta.modified()?.duration_since(UNIX_EPOCH).map(|d| d.as_micros() as u64).unwrap_or(0);
    Ok(Stamp { len: meta.len(), modified_us })
}

pub fn read(path: &Path) -> Result<TextFile, TextError> {
    let stamp = stamp(path)?;
    if stamp.len > MAX_TEXT_BYTES {
        return Err(TextError::TooBig);
    }
    let (text, encoding) = decode(&fs::read(path)?)?;
    let line_ending = if text.matches("\r\n").count() * 2 > text.matches('\n').count() { LineEnding::Crlf } else { LineEnding::Lf };
    Ok(TextFile { text: text.replace("\r\n", "\n"), encoding, line_ending, stamp })
}

/// Writes `file.text` with its encoding and line endings, through a temporary file renamed over the
/// original. Refuses if the file changed since `file.stamp`, unless `force`. Returns the new stamp.
pub fn write(path: &Path, file: &TextFile, force: bool) -> Result<Stamp, TextError> {
    if !force && path.exists() && stamp(path)? != file.stamp {
        return Err(TextError::Changed);
    }
    let text = match file.line_ending {
        LineEnding::Crlf => file.text.replace("\r\n", "\n").replace('\n', "\r\n"),
        LineEnding::Lf => file.text.clone(),
    };
    let bytes = encode(&text, file.encoding)?;
    let tmp = temp_path(path);
    let result = (|| {
        let mut out = fs::File::create(&tmp)?;
        out.write_all(&bytes)?;
        out.sync_all()?;
        drop(out);
        // On Windows this is MoveFileExW with REPLACE_EXISTING: the old file stays whole until here.
        fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result?;
    stamp(path)
}

/// `.name.jaye-tmp` next to the file, so the rename stays on the same disk.
fn temp_path(path: &Path) -> PathBuf {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    path.with_file_name(format!(".{name}.jaye-tmp"))
}

fn decode(bytes: &[u8]) -> Result<(String, Encoding), TextError> {
    if let Some(rest) = bytes.strip_prefix(b"\xef\xbb\xbf") {
        return Ok((String::from_utf8_lossy(rest).into_owned(), Encoding::Utf8Bom));
    }
    for (bom, enc, be) in [(b"\xff\xfe", Encoding::Utf16Le, false), (b"\xfe\xff", Encoding::Utf16Be, true)] {
        if let Some(rest) = bytes.strip_prefix(bom) {
            let units = rest.chunks_exact(2).map(|c| if be { u16::from_be_bytes([c[0], c[1]]) } else { u16::from_le_bytes([c[0], c[1]]) });
            return Ok((char::decode_utf16(units).map(|c| c.unwrap_or('\u{fffd}')).collect(), enc));
        }
    }
    if bytes.contains(&0) {
        return Err(TextError::Binary);
    }
    match std::str::from_utf8(bytes) {
        Ok(s) => Ok((s.to_owned(), Encoding::Utf8)),
        Err(_) => Ok((bytes.iter().map(|&b| from_1252(b)).collect(), Encoding::Windows1252)),
    }
}

fn encode(text: &str, encoding: Encoding) -> Result<Vec<u8>, TextError> {
    Ok(match encoding {
        Encoding::Utf8 => text.as_bytes().to_vec(),
        Encoding::Utf8Bom => [b"\xef\xbb\xbf".as_slice(), text.as_bytes()].concat(),
        Encoding::Utf16Le => [0xfeffu16].into_iter().chain(text.encode_utf16()).flat_map(u16::to_le_bytes).collect(),
        Encoding::Utf16Be => [0xfeffu16].into_iter().chain(text.encode_utf16()).flat_map(u16::to_be_bytes).collect(),
        Encoding::Windows1252 => text.chars().map(|c| to_1252(c).ok_or_else(|| TextError::Unencodable { ch: c.to_string() })).collect::<Result<_, _>>()?,
    })
}

/// Windows-1252's 0x80..=0x9F; the 5 undefined bytes map to the C1 control with the same number.
const CP1252_HIGH: [char; 32] = [
    '€', '\u{81}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{8d}', 'Ž', '\u{8f}', //
    '\u{90}', '‘', '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '\u{9d}', 'ž', 'Ÿ',
];

pub(crate) fn from_1252(b: u8) -> char {
    match b {
        0x80..=0x9f => CP1252_HIGH[(b - 0x80) as usize],
        _ => b as char, // the rest is Latin-1
    }
}

fn to_1252(c: char) -> Option<u8> {
    match c as u32 {
        0..=0x7f | 0xa0..=0xff => Some(c as u8),
        _ => CP1252_HIGH.iter().position(|&x| x == c).map(|i| 0x80 + i as u8),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir() -> PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let d = std::env::temp_dir().join(format!("jay-docs-text-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        fs::create_dir_all(&d).unwrap();
        d
    }

    /// Read, change nothing, write: the same bytes.
    fn round_trip(bytes: &[u8]) -> (TextFile, Vec<u8>) {
        let d = dir();
        let p = d.join("a.md");
        fs::write(&p, bytes).unwrap();
        let file = read(&p).unwrap();
        write(&p, &file, false).unwrap();
        let out = fs::read(&p).unwrap();
        fs::remove_dir_all(&d).unwrap();
        (file, out)
    }

    #[test]
    fn every_encoding_round_trips_byte_for_byte() {
        let cases: [(&[u8], Encoding, LineEnding); 6] = [
            (b"# Ol\xc3\xa1\nmundo\n", Encoding::Utf8, LineEnding::Lf),
            (b"\xef\xbb\xbf# Ol\xc3\xa1\r\nmundo\r\n", Encoding::Utf8Bom, LineEnding::Crlf),
            (b"\xff\xfe#\0 \0O\0l\0\xe1\0\r\0\n\0", Encoding::Utf16Le, LineEnding::Crlf),
            (b"\xfe\xff\0#\0 \0O\0l\0\xe1\0\n", Encoding::Utf16Be, LineEnding::Lf),
            // "Olá — “aspas” €" in Windows-1252, CRLF.
            (b"Ol\xe1 \x97 \x93aspas\x94 \x80\r\nfim\r\n", Encoding::Windows1252, LineEnding::Crlf),
            // Shift-JIS read as 1252: odd chars on screen, but the bytes survive (0x81 is undefined in 1252).
            (b"\x82\xb1\x82\xf1\x81\x40\n", Encoding::Windows1252, LineEnding::Lf),
        ];
        for (bytes, encoding, line_ending) in cases {
            let (file, out) = round_trip(bytes);
            assert_eq!((file.encoding, file.line_ending), (encoding, line_ending), "{bytes:?}");
            assert!(!file.text.contains('\r'), "{:?}", file.text);
            assert_eq!(out, bytes, "{encoding:?}");
        }
        assert_eq!(round_trip(b"Ol\xe1 \x97 \x93aspas\x94 \x80\r\n").0.text, "Olá — “aspas” €\n");
    }

    #[test]
    fn edits_keep_the_file_s_encoding_and_line_endings() {
        let d = dir();
        let p = d.join("a.txt");
        fs::write(&p, b"linha 1\r\nlinha 2\r\n").unwrap();
        let mut file = read(&p).unwrap();
        file.text.push_str("ação\n");
        file.encoding = Encoding::Windows1252;
        file.stamp = write(&p, &file, false).unwrap();
        assert_eq!(fs::read(&p).unwrap(), b"linha 1\r\nlinha 2\r\na\xe7\xe3o\r\n");
        // A char 1252 can't hold is refused, and the file stays as it was.
        file.text.push_str("日本\n");
        assert_eq!(write(&p, &file, false), Err(TextError::Unencodable { ch: "日".into() }));
        assert_eq!(fs::read(&p).unwrap(), b"linha 1\r\nlinha 2\r\na\xe7\xe3o\r\n");
        assert!(!temp_path(&p).exists());
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn a_file_changed_elsewhere_is_not_overwritten() {
        let d = dir();
        let p = d.join("a.md");
        fs::write(&p, "mine\n").unwrap();
        let file = read(&p).unwrap();
        // The page keeps the stamp as a JS number: it must stay exact (below 2^53).
        assert!(file.stamp.modified_us > 0 && file.stamp.modified_us < 1 << 53);
        fs::write(&p, "theirs, longer\n").unwrap();
        assert_eq!(write(&p, &file, false), Err(TextError::Changed));
        assert_eq!(fs::read_to_string(&p).unwrap(), "theirs, longer\n");
        write(&p, &file, true).unwrap();
        assert_eq!(fs::read_to_string(&p).unwrap(), "mine\n");
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn not_text() {
        let d = dir();
        let p = d.join("a.bin");
        fs::write(&p, b"abc\0def").unwrap();
        assert_eq!(read(&p), Err(TextError::Binary));
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn cp1252_table_is_a_bijection_on_bytes() {
        for b in 0..=255u8 {
            assert_eq!(to_1252(from_1252(b)), Some(b), "{b:#x}");
        }
    }
}
