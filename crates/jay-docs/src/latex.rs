//! LaTeX to PDF with Tectonic (MIT; XeTeX inside). Decision D6: not in the installer (TeX would
//! be hundreds of MB); downloaded when the person asks, from a fixed release address, checked
//! against its SHA-256 (as Jay-H does with installers). Tectonic itself then fetches only the TeX
//! packages a document uses, and caches them.
//!
//! Untrusted documents: Tectonic runs in its untrusted mode (no shell escape), with no window,
//! killed after a time limit or when cancelled. The PDF goes to a cache folder; the source folder
//! is only read (`-Z search-path`, for images and .bib files beside the .tex).

use serde::Serialize;
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

pub const TECTONIC_VERSION: &str = "0.17.0";
pub const TECTONIC_URL: &str = "https://github.com/tectonic-typesetting/tectonic/releases/download/tectonic%400.17.0/tectonic-0.17.0-x86_64-pc-windows-msvc.zip";
pub const TECTONIC_SHA256: &str = "f61ce51f0b0ade1015b7de7ef368541c5424e9756ecbd0d7af97d6d48030845f";
pub const TECTONIC_ZIP_BYTES: u64 = 21_060_223;
/// The longest a compile may take (the first one also downloads the packages it needs).
pub const COMPILE_LIMIT: Duration = Duration::from_secs(600);

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum LatexError {
    /// Tectonic isn't installed.
    Missing,
    /// The download wasn't the file expected (size or SHA-256).
    Checksum,
    Download { message: String },
    Timeout,
    Cancelled,
    Io { message: String },
}

impl From<std::io::Error> for LatexError {
    fn from(e: std::io::Error) -> Self {
        Self::Io { message: e.to_string() }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Diagnostic {
    /// 1-based line in the .tex, when TeX says.
    pub line: Option<u32>,
    pub message: String,
    pub warning: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Compiled {
    /// The PDF, when TeX produced one (it can, even with errors).
    pub pdf: Option<PathBuf>,
    pub diagnostics: Vec<Diagnostic>,
    pub seconds: f64,
}

/// Where Tectonic lives inside `dir` (the app's data folder).
pub fn tectonic_path(dir: &Path) -> PathBuf {
    dir.join("tectonic").join(format!("tectonic-{TECTONIC_VERSION}.exe"))
}

/// Downloads Tectonic with Windows' own curl (no HTTP library in the app), checks it, and puts the
/// executable at `tectonic_path(dir)`.
pub fn install(dir: &Path) -> Result<PathBuf, LatexError> {
    let folder = dir.join("tectonic");
    std::fs::create_dir_all(&folder)?;
    let zip = folder.join("download.zip.jaye-tmp");
    // System32's curl by full path: not one found on PATH.
    let curl = std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows")).join("System32").join("curl.exe");
    let status = no_window(Command::new(curl).args(["-fsSL", "--proto", "=https", "--max-filesize", "64000000", "-o"]).arg(&zip).arg(TECTONIC_URL))
        .stdin(Stdio::null())
        .status();
    let result = match status {
        Ok(s) if s.success() => std::fs::read(&zip).map_err(LatexError::from).and_then(|bytes| unpack(&bytes, &folder)),
        Ok(s) => Err(LatexError::Download { message: format!("curl exited with {s}") }),
        Err(e) => Err(LatexError::Download { message: e.to_string() }),
    };
    let _ = std::fs::remove_file(&zip);
    result
}

/// Checks the download's size and SHA-256, then takes `tectonic.exe` out of it.
fn unpack(bytes: &[u8], folder: &Path) -> Result<PathBuf, LatexError> {
    if bytes.len() as u64 != TECTONIC_ZIP_BYTES || hex(&Sha256::digest(bytes)) != TECTONIC_SHA256 {
        return Err(LatexError::Checksum);
    }
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|e| LatexError::Download { message: e.to_string() })?;
    let mut exe = zip.by_name("tectonic.exe").map_err(|e| LatexError::Download { message: e.to_string() })?;
    let mut out = Vec::new();
    (&mut exe).take(256 << 20).read_to_end(&mut out)?;
    let target = folder.join(format!("tectonic-{TECTONIC_VERSION}.exe"));
    let tmp = target.with_extension("exe.jaye-tmp");
    std::fs::write(&tmp, out)?;
    std::fs::rename(&tmp, &target)?;
    Ok(target)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn no_window(cmd: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    cmd
}

/// Compiles `source` (or `text`, the editor's unsaved version of it) into `out_dir`.
pub fn compile(tectonic: &Path, source: &Path, text: Option<&str>, out_dir: &Path, cancel: &AtomicBool) -> Result<Compiled, LatexError> {
    if !tectonic.is_file() {
        return Err(LatexError::Missing);
    }
    std::fs::create_dir_all(out_dir)?;
    let stem = source.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "document".into());
    // Unsaved text goes in a copy in the output folder; the source folder stays on the search path.
    let input = match text {
        Some(t) => {
            let copy = out_dir.join(format!("{stem}.tex"));
            std::fs::write(&copy, t)?;
            copy
        }
        None => source.to_path_buf(),
    };
    let pdf = out_dir.join(format!("{stem}.pdf"));
    let _ = std::fs::remove_file(&pdf);
    let search = source.parent().unwrap_or(Path::new("."));
    let started = Instant::now();
    let mut child = no_window(&mut Command::new(tectonic))
        .args(["--keep-logs", "--chatter", "minimal", "--outdir"])
        .arg(out_dir)
        .arg("-Z")
        .arg(format!("search-path={}", search.display()))
        .arg(&input)
        // Untrusted mode (no shell escape), by the documented variable.
        .env("TECTONIC_UNTRUSTED_MODE", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;
    // Read stderr on the side, so a full pipe never blocks Tectonic.
    let mut stderr = child.stderr.take();
    let reader = std::thread::spawn(move || {
        let mut s = String::new();
        if let Some(e) = stderr.as_mut() {
            let _ = e.take(4 << 20).read_to_string(&mut s);
        }
        s
    });
    loop {
        if child.try_wait()?.is_some() {
            break;
        }
        if cancel.load(Ordering::Relaxed) || started.elapsed() > COMPILE_LIMIT {
            let _ = child.kill();
            let _ = child.wait();
            return Err(if cancel.load(Ordering::Relaxed) { LatexError::Cancelled } else { LatexError::Timeout });
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let stderr = reader.join().unwrap_or_default();
    let log = std::fs::read_to_string(out_dir.join(format!("{stem}.log"))).unwrap_or_default();
    let mut diagnostics = from_stderr(&stderr);
    if !diagnostics.iter().any(|d| !d.warning && d.line.is_some()) {
        diagnostics.extend(from_log(&log));
    }
    Ok(Compiled { pdf: pdf.is_file().then_some(pdf), diagnostics, seconds: started.elapsed().as_secs_f64() })
}

/// Tectonic's own messages: "error: file.tex:12: Undefined control sequence" and "warning: …".
fn from_stderr(stderr: &str) -> Vec<Diagnostic> {
    let mut out = vec![];
    for line in stderr.lines() {
        let (warning, rest) = match (line.strip_prefix("error: "), line.strip_prefix("warning: ")) {
            (Some(r), _) => (false, r),
            (_, Some(r)) => (true, r),
            _ => continue,
        };
        // "<file>:<line>: <message>", the file possibly with a drive's colon in it.
        let parsed = rest.match_indices(':').find_map(|(i, _)| {
            let after = &rest[i + 1..];
            let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
            let tail = after[digits.len()..].strip_prefix(": ")?;
            (!digits.is_empty()).then(|| (digits.parse().ok(), tail.to_owned()))
        });
        let (line_no, message) = parsed.unwrap_or((None, rest.to_owned()));
        if !message.trim().is_empty() {
            out.push(Diagnostic { line: line_no, message: message.trim().to_owned(), warning });
        }
    }
    out
}

/// The TeX log: "! Message." then, a few lines below, "l.12 the source line".
fn from_log(log: &str) -> Vec<Diagnostic> {
    let lines: Vec<&str> = log.lines().collect();
    let mut out = vec![];
    for (i, l) in lines.iter().enumerate() {
        if let Some(message) = l.strip_prefix("! ") {
            let line = lines[i + 1..].iter().take(12).find_map(|n| n.strip_prefix("l.")).and_then(|n| n.split(|c: char| !c.is_ascii_digit()).next()?.parse().ok());
            out.push(Diagnostic { line, message: message.trim().to_owned(), warning: false });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tectonic_messages() {
        let stderr = "note: Running TeX ...\nerror: C:\\Users\\me\\paper.tex:12: Undefined control sequence\nwarning: paper.tex:30: Overfull \\hbox (3.0pt too wide) in paragraph\nerror: halted on potentially-recoverable error as specified\n";
        assert_eq!(
            from_stderr(stderr),
            vec![
                Diagnostic { line: Some(12), message: "Undefined control sequence".into(), warning: false },
                Diagnostic { line: Some(30), message: "Overfull \\hbox (3.0pt too wide) in paragraph".into(), warning: true },
                Diagnostic { line: None, message: "halted on potentially-recoverable error as specified".into(), warning: false },
            ]
        );
    }

    #[test]
    fn tex_log_errors() {
        let log = "(./paper.tex\n! Undefined control sequence.\nl.7 \\foo\n         {bar}\n! Missing $ inserted.\n<inserted text>\n                $\nl.15 a_b\n";
        assert_eq!(
            from_log(log),
            vec![Diagnostic { line: Some(7), message: "Undefined control sequence.".into(), warning: false }, Diagnostic { line: Some(15), message: "Missing $ inserted.".into(), warning: false }]
        );
    }

    #[test]
    fn a_wrong_download_is_refused() {
        let dir = std::env::temp_dir().join(format!("jay-latex-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(unpack(b"not the release", &dir).err(), Some(LatexError::Checksum));
        // Right size, wrong bytes.
        assert_eq!(unpack(&vec![0u8; TECTONIC_ZIP_BYTES as usize], &dir).err(), Some(LatexError::Checksum));
        assert!(!tectonic_path(&dir).exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn missing_tectonic() {
        let r = compile(Path::new("C:/nowhere/tectonic.exe"), Path::new("a.tex"), None, &std::env::temp_dir(), &AtomicBool::new(false));
        assert_eq!(r.err(), Some(LatexError::Missing));
    }
}
